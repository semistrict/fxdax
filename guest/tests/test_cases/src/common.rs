// Copyright The libkrun Authors.
// Copyright 2026 The fxdax Authors: changed for fxdax's guest tests.
// SPDX-License-Identifier: Apache-2.0

//! Common utilities used by multiple test

use anyhow::Context;
use std::fs;
use std::fs::create_dir;
use std::path::{Path, PathBuf};

use crate::{ShouldRun, TestSetup};

fn copy_guest_agent(dir: &Path) -> anyhow::Result<()> {
    let path = std::env::var_os("FXDAX_GUEST_AGENT")
        .context("FXDAX_GUEST_AGENT does not name the guest agent")?;

    let output_path = dir.join("guest-agent");
    fs::copy(path, output_path).context("Failed to copy executable into vm")?;
    Ok(())
}

/// Creates the root filesystem directory, if the runner didn't, and copies the guest agent into
/// it. Returns the path to the root directory.
pub fn setup_rootfs(test_setup: &TestSetup) -> anyhow::Result<PathBuf> {
    let root_dir = test_setup.tmp_dir.join("rootfs");
    if !root_dir.exists() {
        create_dir(&root_dir).context("Failed to create rootfs directory")?;
    }
    copy_guest_agent(&root_dir)?;
    Ok(root_dir)
}

/// Whether a test of fxdax's guest kernel's Fxfs runs: FXDAX_FIRECRACKER names the VMM, and
/// FXDAX_GUEST_KERNEL the kernel, and FXDAX_EXTENTD the socket of the extent daemon that serves
/// the device's store (as fxdax's scripts/test-e2e.sh has them).
pub fn fxdax_guest_should_run() -> ShouldRun {
    if !crate::vm::has_vmm() {
        return ShouldRun::No("FXDAX_FIRECRACKER does not name fxdax's Firecracker");
    }
    if std::env::var_os("FXDAX_GUEST_KERNEL").is_none() {
        return ShouldRun::No("FXDAX_GUEST_KERNEL does not name a guest kernel");
    }
    if std::env::var_os("FXDAX_EXTENTD").is_none() {
        return ShouldRun::No("FXDAX_EXTENTD does not name the extent daemon's socket");
    }
    ShouldRun::Yes
}

/// fxdax: how many times slower than a release one the guest kernel is, by its kind,
/// FXDAX_GUEST_KERNEL_BUILD's, as scripts/lib/test-linux-host.sh built it (a release one if
/// unset): what a test's time limits scale by (TASK-168). The debug kernel's measured: its checks
/// of a second or more took 1.37 times the release kernel's at most (GCE r561, r562 against r560,
/// r563). The others' guessed, KASAN's from its outrunning the checks' 180 s where the release
/// kernel took 140 s or so (TASK-44.8).
pub fn guest_kernel_slowness() -> anyhow::Result<f64> {
    match std::env::var("FXDAX_GUEST_KERNEL_BUILD").unwrap_or_default().as_str() {
        "" | "release" => Ok(1.0),
        "debug" | "rust-checks" | "coverage" => Ok(1.5),
        "kasan" => Ok(4.0),
        kind => anyhow::bail!(
            "FXDAX_GUEST_KERNEL_BUILD is release, debug, kasan, coverage or rust-checks, not {kind:?}"
        ),
    }
}

/// fxdax: says in the round's log what the daemon named `name` the test spawned ([`spawn_daemon`])
/// kept in its stats file, how long its commits waited to be made durable among it: a passing
/// round's directory, and the file in it, doesn't outlive the run (TASK-165.9).
pub fn say_daemon_stats(test_setup: &TestSetup, name: &str) {
    let path = test_setup.tmp_dir.join(format!("{name}-stats.txt"));
    match fs::read_to_string(&path) {
        Ok(kept) => kept.lines().for_each(|line| eprintln!("{name} stats: {line}")),
        Err(err) => eprintln!("{name} stats: reading {}: {err}", path.display()),
    }
    // And its stores' warnings, of flushes and framings past extentd's turn among them, in its
    // log, as the stats file, gone with a round's directory that passed.
    let log = test_setup.tmp_dir.join(format!("{name}.log"));
    let warned = fs::read_to_string(&log).unwrap_or_default();
    let warned = warned.lines().filter(|line| line.contains("WARN extent_store"));
    for line in warned.take(200) {
        eprintln!("{name} log: {line}");
    }
}

/// fxdax: fails unless the daemon named `name` the test spawned made half its stores' commits
/// durable within extentd's turn, 50 ms, at least, as its stats file says: doc-12's median
/// (`durable_lag_within_50ms`, TASK-221).
pub fn check_durable_median(test_setup: &TestSetup, name: &str) -> anyhow::Result<()> {
    let path = test_setup.tmp_dir.join(format!("{name}-stats.txt"));
    let kept = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let daemon = kept.lines().find_map(|line| line.strip_prefix("daemon: "));
    let daemon = daemon.context("no daemon line")?;
    let field = |name: &str| -> anyhow::Result<u64> {
        let value = daemon.split(' ').find_map(|field| field.strip_prefix(name)?.strip_prefix('='));
        Ok(value.with_context(|| format!("no {name}"))?.parse()?)
    };
    let (count, within) = (field("durable_lag_count")?, field("durable_lag_within_50ms")?);
    anyhow::ensure!(
        2 * within >= count,
        "{within} of {count} commits durable within 50 ms: doc-12's median is 50 ms"
    );
    Ok(())
}

/// fxdax: the memory a test's extent daemon takes at most unless its case says (`MEMORY_BYTES`),
/// the sandbox team's daemon's: room for what its VMs read again, as a host's page cache holds a
/// virtio-pmem image's (TASK-227). The daemon's own default, 1 GiB, held 1 GiB of a 1.54 GB Linux
/// tree, which its warm reread through an 8 GiB window paged in again, 13.15 s; 8 GiB, 600 ms
/// (GCE r880, r881). A bound, not a reservation: the daemon takes pages as it caches them.
pub const DAEMON_MEMORY: u64 = 8 << 30;

/// fxdax: the most bytes a DAX window has, the extent daemon's (`extentd::wire::MAX_WINDOW`): what
/// it keeps for a window grows with the pages it maps, which it bounds so (TASK-150.3).
pub const MAX_DAX_WINDOW: u64 = 64 << 30;

/// fxdax: a case's DAX window: as large as the data, the host's memory, or `FXDAX_DAX_WINDOW_MIB`'s
/// if set ([`Window::Large`]); or a small window's, whose cases check it fills
/// ([`Window::Small`]), as fxfs-guest-checks-dax-small's, which a large one fails (TASK-227).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Large,
    Small(u64),
}

/// fxdax: the bytes of a case's DAX window `window` asks for ([`Window`]): the window is address
/// space, not memory, the daemon's pages it maps one copy whatever the windows' sizes, as
/// virtio-pmem maps a whole image; its guest makes struct pages for what it reaches alone
/// (TASK-92.8), and its VM process keeps changes for those pages alone (TASK-227).
pub fn dax_window(window: Window) -> anyhow::Result<u64> {
    let given = std::env::var("FXDAX_DAX_WINDOW_MIB").ok();
    let meminfo = std::fs::read_to_string("/proc/meminfo")?;
    window_bytes(window, given.as_deref(), &meminfo)
}

/// fxdax: [`dax_window`]'s bytes, of `window`, `given` FXDAX_DAX_WINDOW_MIB's, and the host's
/// /proc/meminfo, `meminfo`: a large window's `given`'s, or the host's MemTotal, rounded down to a
/// window's 2 MiB, at most [`MAX_DAX_WINDOW`].
fn window_bytes(window: Window, given: Option<&str>, meminfo: &str) -> anyhow::Result<u64> {
    const ALIGNMENT: u64 = 2 << 20;
    match window {
        Window::Small(bytes) => Ok(bytes),
        Window::Large if let Some(mib) = given.filter(|mib| !mib.is_empty()) => {
            Ok(mib.parse::<u64>().context("FXDAX_DAX_WINDOW_MIB")? << 20)
        }
        Window::Large => {
            let line = (meminfo.lines().find(|line| line.starts_with("MemTotal:")))
                .context("/proc/meminfo has no MemTotal")?;
            let kib: u64 =
                (line.split_whitespace().nth(1)).context("MemTotal has no value")?.parse()?;
            Ok(((kib << 10) / ALIGNMENT * ALIGNMENT).min(MAX_DAX_WINDOW))
        }
    }
}

/// An extent daemon of the test's own, FXDAX_EXTENTD_BIN, serving at `socket`, and its control API
/// beside it ([`control`]), once this returns, with `env` in its environment, and `memory` bytes of
/// memory if given, [`DAEMON_MEMORY`] otherwise, logging to `<name>.log` in the test's directory.
/// fxdax: it keeps what its stores read from their bucket in a part cache of its own,
/// `<name>-cache` in the test's directory, as production's daemon keeps it on the host's disk
/// (`FXDAX_EXTENTD_CACHE`, doc-7), unless `env` or this process says otherwise, empty for none: one
/// daemon at a time has a cache's directory (TASK-164.12). And it keeps what it did in
/// `<name>-stats.txt` beside its log, unless `env` names a file (`FXDAX_EXTENTD_STATS`): what it
/// downloaded against what it read, which the run's summary says (scripts/guest_results.py,
/// TASK-165.12).
pub fn spawn_daemon(
    test_setup: &TestSetup,
    socket: &Path,
    name: &str,
    env: &[(&str, &std::ffi::OsStr)],
    memory: Option<u64>,
) -> anyhow::Result<std::process::Child> {
    use std::time::{Duration, Instant};

    let bin = std::env::var_os("FXDAX_EXTENTD_BIN").context("FXDAX_EXTENTD_BIN")?;
    let log = fs::File::create(test_setup.tmp_dir.join(format!("{name}.log")))?;
    // A host keeps stores in a bucket, `env`'s or this process's, or a file bucket of its own in
    // the test's directory, as of placement 1, unless one is named. Not one in its memory: the
    // cases open the stores it made in processes of their own, after it, which keep a store's
    // branch in the bucket its description names, and opening one whose branch the bucket lacks,
    // its commits acknowledged, is refused (TASK-165.9), as scripts/test-e2e.sh's daemon's.
    let named = |var: &str| {
        env.iter().any(|(given, value)| *given == var && !value.is_empty())
            || std::env::var_os(var).is_some_and(|value| !value.is_empty())
    };
    let mut command = std::process::Command::new(bin);
    command.arg(socket).env("FXDAX_EXTENTD_GRPC", socket.with_extension("control"));
    command.arg(memory.unwrap_or(DAEMON_MEMORY).to_string());
    if !named("FXDAX_EXTENTD_REMOTE") {
        let bucket = test_setup.tmp_dir.join(format!("{name}.bucket"));
        command.env("FXDAX_EXTENTD_REMOTE", format!("file://{}/", bucket.display()));
    }
    if !named("FXDAX_EXTENTD_TOKEN") {
        command.env("FXDAX_EXTENTD_TOKEN", "1");
    }
    let given = env.iter().find(|(var, _)| *var == "FXDAX_EXTENTD_CACHE").map(|(_, value)| *value);
    let ours = std::env::var_os("FXDAX_EXTENTD_CACHE");
    if let Some(cache) = part_cache(given, ours.as_deref(), &test_setup.tmp_dir, name) {
        fs::create_dir_all(&cache)?;
        command.env("FXDAX_EXTENTD_CACHE", cache);
    }
    let given = env.iter().any(|(var, _)| *var == "FXDAX_EXTENTD_STATS");
    if let Some(stats) = stats_file(given, &test_setup.tmp_dir, name) {
        command.env("FXDAX_EXTENTD_STATS", stats);
    }
    let daemon = command.envs(env.iter().copied()).stderr(log).spawn()?;
    test_setup.register_cleanup_pid(daemon.id());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match control(socket) {
            Ok(_) => {
                give_to_the_jail(socket)?;
                return Ok(daemon);
            }
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(anyhow::anyhow!("the daemon {name}: {err:#}")),
        }
    }
}

/// fxdax: the part cache a test's daemon `name` is given, in `dir`, the test's directory, as
/// production's daemon keeps one on the host's disk (`FXDAX_EXTENTD_CACHE`, doc-7), unless
/// `FXDAX_EXTENTD_CACHE` is said, `given` the daemon or `ours` this process, which it then takes
/// as it is, empty for none. Without one, a fork paged in what it inherited from the bucket a ranged
/// GET at a time: forked-dax 0.304 in the perf gate, 0.849 with it (TASK-164.12, GCE r790, r792).
fn part_cache(
    given: Option<&std::ffi::OsStr>,
    ours: Option<&std::ffi::OsStr>,
    dir: &Path,
    name: &str,
) -> Option<PathBuf> {
    (given.is_none() && ours.is_none()).then(|| dir.join(format!("{name}-cache")))
}

/// fxdax: the stats file a test's daemon `name` keeps what it did in, `<name>-stats.txt` in `dir`,
/// the test's directory, beside its log, unless `env` names one (`given`): what the run's summary
/// reads of each daemon, its downloads against its reads (scripts/guest_results.py, TASK-165.12).
/// Not this process's, which scripts/test-e2e.sh's own daemon alone has, its file the run's.
fn stats_file(given: bool, dir: &Path, name: &str) -> Option<PathBuf> {
    (!given).then(|| dir.join(format!("{name}-stats.txt")))
}

/// Gives the extent daemon's socket at `socket`, its user's alone (TASK-155), to the user of the
/// jails Firecracker runs in, if it does: scripts/test-e2e.sh's FXDAX_FIRECRACKER_JAIL. Each
/// jailed Firecracker here uses the default tenant's socket, as production's use a tenant's.
pub fn give_to_the_jail(socket: &Path) -> anyhow::Result<()> {
    let id = |name: &str| -> anyhow::Result<Option<u32>> {
        let Ok(value) = std::env::var(name) else { return Ok(None) };
        Ok(Some(value.parse().with_context(|| format!("{name} is not a number"))?))
    };
    if let (Some(uid), Some(gid)) = (id("FXDAX_JAIL_UID")?, id("FXDAX_JAIL_GID")?) {
        std::os::unix::fs::chown(socket, Some(uid), Some(gid))
            .with_context(|| format!("giving {} to the jail's user", socket.display()))?;
    }
    Ok(())
}

/// A client of the control API of the extent daemon whose VM socket is at `socket`: at its control
/// socket, beside it, `<socket>.control` for `<socket>.sock`, as fxdax's scripts/test-e2e.sh and
/// [`spawn_daemon`] place it.
pub fn control(socket: &Path) -> Result<fxdax::extentd::control::Client, fxdax::Error> {
    fxdax::extentd::control::Client::connect(socket.with_extension("control"))
}

/// Copies the extent store at `from` to `to`, as [`copy_dir`] does, once no process has it open: its
/// directory taken, as a process opening it takes it, which the extent daemon serving it holds
/// until its last user, an upload too, lets it go. Copied while the daemon held it, an upload's
/// marker's `.new` file was renamed away between listing it and copying it (TASK-129).
pub fn copy_store(from: &Path, to: &Path) -> anyhow::Result<()> {
    use extent_store::Dir;
    let dir = extent_store::HostDir::new(from, false)
        .with_context(|| format!("opening {}", from.display()))?;
    dir.take().with_context(|| format!("taking {}", from.display()))?;
    let copied = copy_dir(from, to);
    dir.let_go();
    copied
}

/// Copies the directory `from` to `to`, which must not exist, with everything in it.
pub fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    create_dir(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// TASK-164.12: a test's daemon keeps a part cache of its own in the test's directory, as
    /// production's does, unless FXDAX_EXTENTD_CACHE is said, by the daemon's environment or this
    /// process's, which it takes as it is, empty for none.
    #[test]
    fn daemons_keep_a_part_cache_unless_told_otherwise() {
        let dir = Path::new("/tmp/case");
        let cache = |given: Option<&str>, ours: Option<&str>| {
            part_cache(given.map(OsStr::new), ours.map(OsStr::new), dir, "extentd")
        };
        assert_eq!(cache(None, None), Some(PathBuf::from("/tmp/case/extentd-cache")));
        assert_eq!(cache(Some(""), None), None);
        assert_eq!(cache(Some("/nvme/cache"), None), None);
        assert_eq!(cache(None, Some("")), None);
        assert_eq!(cache(None, Some("/nvme/cache")), None);
    }

    /// TASK-227: a case's DAX window is the host's memory, rounded down to 2 MiB, at most a
    /// window's 64 GiB, or FXDAX_DAX_WINDOW_MIB's, unless it is a small window's, which stays so:
    /// fxfs-guest-checks-dax-small's window_filled failed in 64 GiB (GCE r886).
    #[test]
    fn windows_are_as_large_as_the_hosts_memory() {
        let host = |kib: u64| format!("MemTotal:       {kib} kB\nMemFree:  1 kB\n");
        let (gib, mib) = (1 << 30, 1 << 20);
        let large = |kib| window_bytes(Window::Large, None, &host(kib)).unwrap();
        assert_eq!(large(32_853_144), 31 * gib + 338 * mib);
        assert_eq!(large(256 << 20), MAX_DAX_WINDOW);
        let small = Window::Small(64 * mib);
        assert_eq!(window_bytes(small, None, &host(1 << 25)).unwrap(), 64 * mib);
        assert_eq!(window_bytes(small, Some("8192"), &host(1 << 25)).unwrap(), 64 * mib);
        assert_eq!(window_bytes(Window::Large, Some("1024"), &host(1 << 25)).unwrap(), gib);
        assert_eq!(window_bytes(Window::Large, Some(""), &host(4 << 20)).unwrap(), 4 * gib);
        assert!(window_bytes(Window::Large, None, "MemFree: 1 kB\n").is_err());
    }

    /// TASK-165.12: a test's daemon keeps its stats beside its log, where the run's summary reads
    /// them, unless its environment names a file, as pgbench's daemons' does.
    #[test]
    fn daemons_keep_their_stats_beside_their_logs() {
        let dir = Path::new("/tmp/case");
        assert_eq!(
            stats_file(false, dir, "extentd"),
            Some(PathBuf::from("/tmp/case/extentd-stats.txt"))
        );
        assert_eq!(stats_file(true, dir, "extentd"), None);
    }
}
