// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the daemon's memory as a page cache (TASK-358): as Linux's page cache, it has no size of
//! its own, but takes whatever of the host's memory is free, and gives it back as others need it.
//! Under DAX, the daemon's memory is a guest's only cache of its files, and fxdax is the only thing
//! on its hosts: memory left free is memory wasted.
//!
//! A thread looks at the host's memory every [`INTERVAL`]: what is available to the daemon, the
//! host's (`MemAvailable`) or, if less, its cgroups', each ancestor's limit less what it holds
//! but could reclaim. The daemon may hold what it holds and that, less a headroom left for VMs'
//! memory to grow into between looks and for the host ([`headroom`]). It sets the limit on every
//! tenant's memory together to that ([`super::memory::govern`]); held past it, the daemon evicts at
//! once, each tenant first down to its share of it by weight ([`shares`]), then any, least recently
//! used first.
//!
//! A daemon given its memory's size (`fxdax-extentd SOCKET MEMORY_BYTES`) is not governed: its
//! memory is that, as a test's daemon's may be.

use super::memory;
use physmem::CHUNK_SIZE;
use std::time::Duration;

/// How often the governor looks at the host's memory.
pub const INTERVAL: Duration = Duration::from_millis(100);

/// What the governor leaves free of the memory available to the daemon:
/// `FXDAX_EXTENTD_HEADROOM` bytes, or a sixteenth of the host's memory, 1 GiB at least: room for
/// VMs' memory to grow into between looks, and for the host's own.
pub fn headroom(total: u64) -> Result<u64, String> {
    match std::env::var("FXDAX_EXTENTD_HEADROOM") {
        Ok(bytes) if !bytes.is_empty() => {
            bytes.parse().map_err(|_| format!("FXDAX_EXTENTD_HEADROOM is bytes, not {bytes:?}"))
        }
        _ => Ok((total / 16).max(1 << 30)),
    }
}

/// The host's memory, as the daemon may use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostMemory {
    /// All of it.
    pub total: u64,
    /// What is available to the daemon now: free, or reclaimable, the host's and its cgroups'.
    pub available: u64,
}

/// The host's memory now (Linux: /proc/meminfo, and the daemon's cgroups').
#[cfg(target_os = "linux")]
pub fn host_memory() -> std::io::Result<HostMemory> {
    let meminfo = std::fs::read_to_string("/proc/meminfo")?;
    let (total, available) = meminfo_of(&meminfo).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "/proc/meminfo: no MemTotal, MemAvailable",
        )
    })?;
    let cgroups = std::fs::read_to_string("/proc/self/cgroup")?;
    let mut available = available;
    if let Some(path) = cgroups.lines().find_map(|line| line.strip_prefix("0::")) {
        let mut dir = std::path::Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        loop {
            if let Some(room) = cgroup_room(&dir)? {
                available = available.min(room);
            }
            if !dir.pop() || dir == std::path::Path::new("/sys/fs") {
                break;
            }
        }
    }
    Ok(HostMemory { total, available })
}

/// The host's memory now (macOS: its pages free and speculative, as the VM statistics count them).
#[cfg(target_os = "macos")]
pub fn host_memory() -> std::io::Result<HostMemory> {
    let page = physmem::page_size();
    let total = sysctl_u64(c"hw.memsize")?;
    let free = sysctl_u64(c"vm.page_free_count")? + sysctl_u64(c"vm.page_speculative_count")?;
    Ok(HostMemory { total, available: free * page })
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &std::ffi::CStr) -> std::io::Result<u64> {
    let mut value = [0u8; 8];
    let mut len = value.len();
    // SAFETY: a sysctl read into `value`, of `len` bytes, which it says how many it filled.
    let read = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(match len {
        4 => u32::from_ne_bytes(value[..4].try_into().unwrap()).into(),
        _ => u64::from_ne_bytes(value),
    })
}

/// /proc/meminfo's MemTotal and MemAvailable, in bytes.
pub fn meminfo_of(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        let line = meminfo.lines().find_map(|line| line.strip_prefix(name))?;
        let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
        Some(kib * 1024)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// What the cgroup at `dir` leaves room for, if it is limited: its limit (memory.max) less what it
/// holds (memory.current), of which its inactive files' pages (memory.stat's inactive_file) are
/// reclaimable.
#[cfg(target_os = "linux")]
fn cgroup_room(dir: &std::path::Path) -> std::io::Result<Option<u64>> {
    let read = |file: &str| match std::fs::read_to_string(dir.join(file)) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    };
    let (Some(max), Some(current), Some(stat)) =
        (read("memory.max")?, read("memory.current")?, read("memory.stat")?)
    else {
        return Ok(None);
    };
    Ok(room_of(&max, &current, &stat))
}

/// A cgroup's room, of its memory.max, memory.current and memory.stat: none if unlimited.
pub fn room_of(max: &str, current: &str, stat: &str) -> Option<u64> {
    let max: u64 = max.trim().parse().ok()?;
    let current: u64 = current.trim().parse().ok()?;
    let inactive: u64 = stat
        .lines()
        .find_map(|line| line.strip_prefix("inactive_file "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Some((max + inactive).saturating_sub(current))
}

/// What the daemon may hold, holding `held`, of `host`, leaving `headroom`: whole chunks, one at
/// least.
pub fn allowance(held: u64, host: HostMemory, headroom: u64) -> u64 {
    let chunk = CHUNK_SIZE as u64;
    ((held + host.available).saturating_sub(headroom) / chunk * chunk).max(chunk)
}

/// Each tenant's share of `target` bytes by its weight, of `tenants`' (name, weight): what it keeps
/// under pressure while it uses it. A tenant of weight 0 has none.
pub fn shares(target: u64, tenants: &[(String, u64)]) -> Vec<(String, u64)> {
    let total: u128 = tenants.iter().map(|(_, weight)| u128::from(*weight)).sum();
    tenants
        .iter()
        .map(|(name, weight)| {
            let share = match total {
                0 => 0,
                total => (u128::from(target) * u128::from(*weight) / total) as u64,
            };
            (name.clone(), share)
        })
        .collect()
}

/// Starts the governor, on a thread of its own, for the rest of the daemon's life. Fails if the
/// host's memory can't be read, or the headroom is malformed: a daemon that can't see the host's
/// memory doesn't start rather than run without its cache.
pub fn start() -> Result<(), String> {
    let host = host_memory().map_err(|err| format!("reading the host's memory: {err}"))?;
    let headroom = headroom(host.total)?;
    std::thread::Builder::new()
        .name("memory governor".into())
        .spawn(move || {
            loop {
                match host_memory() {
                    Ok(host) => memory::govern(allowance(physmem::arenas_bytes(), host, headroom)),
                    Err(err) => log::error!("extentd: reading the host's memory: {err}"),
                }
                std::thread::sleep(INTERVAL);
            }
        })
        .map_err(|err| format!("starting the memory governor: {err}"))?;
    Ok(())
}

/// The size of physical memory for a governed daemon: all of the host's, in whole chunks. Fails if
/// the host maps less (`vm.max_map_count` on Linux): a daemon whose cache can't grow into the
/// host's memory doesn't start.
pub fn memory_bytes() -> Result<u64, String> {
    let host = host_memory().map_err(|err| format!("reading the host's memory: {err}"))?;
    let chunk = CHUNK_SIZE as u64;
    let bytes = host.total / chunk * chunk;
    if let Some(max) = physmem::max_size().filter(|&max| bytes > max) {
        return Err(format!(
            "the host's memory, {bytes} bytes, is more than the daemon can map, {max}: raise \
             vm.max_map_count"
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_gives_total_and_available() {
        let meminfo = "MemTotal:       28311740 kB\nMemFree:          512000 kB\n\
                       MemAvailable:   20000000 kB\nBuffers: 1 kB\n";
        assert_eq!(meminfo_of(meminfo), Some((28311740 * 1024, 20000000 * 1024)));
        assert_eq!(meminfo_of("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn a_cgroups_room_counts_its_inactive_files_reclaimable() {
        let stat = "anon 100\nfile 900\ninactive_file 300\nactive_file 600\n";
        assert_eq!(
            room_of("8589934592\n", "8000000000\n", stat),
            Some(8589934592 + 300 - 8000000000)
        );
        assert_eq!(room_of("max\n", "8000000000\n", stat), None);
        assert_eq!(room_of("100\n", "1000\n", stat), Some(0));
    }

    #[test]
    fn the_allowance_is_whats_held_and_available_less_the_headroom() {
        const GIB: u64 = 1 << 30;
        let chunk = CHUNK_SIZE as u64;
        let host = HostMemory { total: 32 * GIB, available: 10 * GIB };
        assert_eq!(allowance(4 * GIB, host, 2 * GIB), 12 * GIB);
        // Squeezed past the headroom: what is held less what is missing.
        let squeezed = HostMemory { total: 32 * GIB, available: GIB };
        assert_eq!(allowance(4 * GIB, squeezed, 2 * GIB), 3 * GIB);
        // A chunk at least, and whole chunks.
        assert_eq!(allowance(0, squeezed, 2 * GIB), chunk);
        assert_eq!(allowance(chunk + 1, HostMemory { total: GIB, available: 0 }, 0), chunk);
    }

    #[test]
    fn shares_go_by_weight() {
        let tenants = [("a".to_owned(), 3), ("b".to_owned(), 1), ("c".to_owned(), 0)];
        assert_eq!(
            shares(400, &tenants),
            [("a".to_owned(), 300), ("b".to_owned(), 100), ("c".to_owned(), 0)]
        );
        assert_eq!(shares(400, &[("z".to_owned(), 0)]), [("z".to_owned(), 0)]);
    }
}
