// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The in-process kernel that files' pages live in.
//!
//! Fxfs keeps file contents in pager-backed VMOs of a port of the Zircon kernel, which runs in the
//! process and is shared by every filesystem in it. Its physical memory is host memory it commits
//! as it allocates pages, and is what files' cached and dirty pages, and DAX windows' pages, come
//! from. As it runs low, the kernel signals memory pressure: filesystems write dirty pages back
//! sooner, and the kernel evicts clean ones.
//!
//! The kernel starts when the process first mounts a filesystem, with the [`Config`] given to
//! [`configure`], once, before then, or the default.

use crate::error::{Errno, Error, Result};
use zircon::boot_options::BootOptions;

const MIB: u64 = 1024 * 1024;

/// The kernel's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Bytes of physical memory: the most host memory files' pages take.
    pub memory_bytes: u64,
    /// Free memory below which pressure is a warning, and filesystems write back more often.
    /// Thresholds are whole MiB, rounded down.
    pub warning_threshold_bytes: u64,
    /// Free memory below which pressure is critical: filesystems write their dirty pages back at
    /// once, and limit how many more are dirtied, and the kernel evicts clean pages.
    pub critical_threshold_bytes: u64,
    /// Free memory below which memory is out, and the process aborts.
    pub out_of_memory_threshold_bytes: u64,
}

impl Default for Config {
    /// Zircon's defaults: 1 GiB of memory, with warning, critical and out of memory thresholds of
    /// 300, 150 and 50 MiB.
    fn default() -> Self {
        let options = BootOptions::default();
        Self {
            memory_bytes: options.memory_bytes,
            warning_threshold_bytes: options.oom_warning_threshold_mb * MIB,
            critical_threshold_bytes: options.oom_critical_threshold_mb * MIB,
            out_of_memory_threshold_bytes: options.oom_out_of_memory_threshold_mb * MIB,
        }
    }
}

impl Config {
    /// `memory_bytes` of memory, with the default's thresholds scaled to it: warning, critical
    /// and out of memory at 300, 150 and 50 MiB a GiB.
    pub fn with_memory(memory_bytes: u64) -> Self {
        let default = Self::default();
        let scale = |threshold: u64| {
            u64::try_from(
                u128::from(threshold) * u128::from(memory_bytes) / u128::from(default.memory_bytes),
            )
            .unwrap_or(u64::MAX)
        };
        Self {
            memory_bytes,
            warning_threshold_bytes: scale(default.warning_threshold_bytes),
            critical_threshold_bytes: scale(default.critical_threshold_bytes),
            out_of_memory_threshold_bytes: scale(default.out_of_memory_threshold_bytes),
        }
    }
}

/// Configures the kernel, which fails with `EBUSY` if it has already been configured or started,
/// and with `EINVAL` unless the thresholds, in whole MiB, decrease below the memory, each more than
/// the watchdog's debounce (1 MiB) below the one before, the out of memory one too above none; or
/// if the memory is more than the host maps (`vm.max_map_count` on Linux, TASK-79.37).
pub fn configure(config: Config) -> Result<(), Error> {
    let defaults = BootOptions::default();
    let in_mib = [
        config.memory_bytes,
        config.warning_threshold_bytes,
        config.critical_threshold_bytes,
        config.out_of_memory_threshold_bytes,
        0,
    ]
    .map(|bytes| bytes / MIB);
    if !in_mib.windows(2).all(|pair| pair[0] > pair[1] + defaults.oom_debounce_mb) {
        return Err(Error::new(
            anyhow::Error::new(Errno::EINVAL)
                .context("memory thresholds must decrease, and be below the memory"),
        ));
    }
    if zircon::vm::physmap::max_size().is_some_and(|max| config.memory_bytes > max) {
        return Err(Error::new(
            anyhow::Error::new(Errno::EINVAL)
                .context("the memory is more than the host maps: raise vm.max_map_count"),
        ));
    }
    let (critical, out_of_memory) = (in_mib[2], in_mib[3]);
    let options = BootOptions {
        memory_bytes: config.memory_bytes,
        oom_warning_threshold_mb: in_mib[1],
        oom_critical_threshold_mb: critical,
        oom_out_of_memory_threshold_mb: out_of_memory,
        // fxdax: imminently out halfway to critical at most, so that a small memory's levels keep
        // their order; none where there is no room between them.
        oom_imminent_oom_delta_mb: Some((critical - out_of_memory) / 2)
            .filter(|delta| *delta > defaults.oom_debounce_mb)
            .map_or(0, |delta| delta.min(defaults.oom_imminent_oom_delta_mb)),
        ..defaults
    };
    BootOptions::set(options).map_err(|_| {
        Error::new(
            anyhow::Error::new(Errno::EBUSY)
                .context("the kernel has already been configured or started"),
        )
    })
}

pub use zircon::vm::physmap::{ArenaId, PhysLocation};

/// The kernel's physical memory of the default arena: a shared memory object (a memfd on Linux),
/// which files' and DAX windows' pages are of, but for tenants' (TASK-43), and which a process
/// sharing it maps them from.
pub fn physical_memory() -> std::os::fd::BorrowedFd<'static> {
    zircon::vm::pmm::node().phys_memory().fd()
}

/// The shared memory object of the kernel's physical memory of `arena`, while it exists.
pub fn arena_memory(arena: ArenaId) -> Option<std::sync::Arc<std::os::fd::OwnedFd>> {
    zircon::vm::pmm::node().phys_memory().arena_fd(arena)
}

/// The kernel's page size, which DAX windows and mappings are aligned to: the host's, such as 16
/// KiB on Apple silicon.
pub fn page_size() -> u64 {
    zx::system_get_page_size() as u64
}
