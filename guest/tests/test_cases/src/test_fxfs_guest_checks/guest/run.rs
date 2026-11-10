// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the agent's running of the checks, each in a process of its own: the agent run again,
//! the check named in its environment ([`ALONE`]), to a deadline, the check's budget (TASK-168),
//! of the guest's time: what the guest waited for the host's answers to its device's requests, the
//! host's time, is left out of it ([`Clock`], TASK-263).
//! A check that crashes, or runs on past its budget, which the agent kills once it has the guest's
//! stacks, fails alone, by name, and the checks after it run (TASK-160.30). What a check's process
//! held goes with it, its mappings and files; what it leaves of the guest's state the agent puts
//! back, in one place, before the next check ([`Guest::put_back`]): fxfs's module parameters as
//! the guest booted with them, the filesystem thawed, and mounted.

use super::*;
use crate::Test;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::mpsc::{self, RecvTimeoutError};

/// In a check's process's environment, the check it runs.
const ALONE: &str = "FXDAX_CHECK";

/// How long a check's processes, killed, may take to end: past it, the guest is too stuck for the
/// checks after to run.
const REAP: Duration = Duration::from_secs(10);

impl Test for TestFxfsGuestChecks {
    fn in_guest(self: Box<Self>) {
        match std::env::var(ALONE) {
            Ok(name) => self.run_alone(&name),
            Err(_) => self.run_all(),
        }
    }
}

impl TestFxfsGuestChecks {
    /// In a check's process: runs the check `name`, which fails as the process ends otherwise
    /// than with 0, as it does as the check panics.
    fn run_alone(&self, name: &str) {
        let at = CHECKS.iter().position(|check| check.name == name).unwrap();
        BODIES[at](self);
    }

    /// In the agent: runs the suite's checks, FXDAX_CHECKS's if it names any, each in a process
    /// of its own, and says OK, or which failed.
    fn run_all(&self) {
        let guest = Guest::set_up();
        let only = std::env::var("FXDAX_CHECKS").unwrap_or_default();
        let checks = self.checks(&only).unwrap();
        let scale: f64 = std::env::var(SCALE).map_or(1.0, |scale| scale.parse().unwrap());
        let mut failed = Vec::new();
        for (_, check) in checks {
            let budget = check.budget.mul_f64(scale);
            let (mut clock, host) = (Clock::start(), HostTook::now());
            let ended = guest.run(check.name, budget, &mut clock);
            // fxdax: and what else of its time was the host's: its CPUs' and its requests'.
            eprintln!("{}: the host took {}", check.name, host.since());
            // fxdax: each check's time, against its budget, and the host's, in the run's log
            // (TASK-168, TASK-263).
            let (took, host) = (clock.guest(), clock.host);
            let times = format!("{took:.2?} of {budget:.2?}, and {host:.2?} the host's");
            if ended == Ended::Passed {
                eprintln!("PASS: {} ({times})", check.name);
            } else {
                eprintln!("FAIL: {} ({times}): {ended}", check.name);
                failed.push(check.name);
            }
        }
        save_coverage();
        fsync_dir(MNT);

        if failed.is_empty() {
            println!("OK");
        } else {
            // What the guest's Fxfs logged.
            crate::kernel_log::print();
            println!("FAILED: {}", failed.join(", "));
        }
    }
}

/// The guest as each check finds it: the image mounted, and fxfs's module parameters as the guest
/// booted with them.
struct Guest {
    /// fxfs's parameters a check may set, those readable and writable, by path, and their values
    /// as the guest booted.
    params: Vec<(PathBuf, String)>,
}

impl Guest {
    /// Mounts the image, and notes fxfs's parameters.
    fn set_up() -> Self {
        // The checks' orphans are the agent's, which waits for them, whatever its process ID.
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
        mount();
        let settable = |entry: &fs::DirEntry| entry.metadata().unwrap().mode() & 0o600 == 0o600;
        let params = (fs::read_dir(PARAMS).unwrap().map(Result::unwrap))
            .filter(settable)
            .map(|entry| (entry.path(), read_param(&entry.path())))
            .collect();
        Self { params }
    }

    /// Runs the check `name` in a process of its own, the agent run again, to `budget` of
    /// `clock`'s time, past which the guest's stacks are said and the check killed; puts back what
    /// it left of the guest; and says how it ended. Resets the VM if its processes don't end once
    /// killed.
    fn run(&self, name: &str, budget: Duration, clock: &mut Clock) -> Ended {
        let mut check = Command::new("/proc/self/exe");
        check.args(std::env::args_os().skip(1)).env(ALONE, name);
        let overran = || {
            let stacks = crate::kernel_log::stacks().concat();
            guest_init::complain_out(&format!(
                "{name}: still running after its budget of {budget:.2?}; the guest's stacks:\n\
                 {stacks}"
            ));
        };
        match run_to(check, budget, || clock.spent(), overran, || self.put_back(name)) {
            Ok(ended) => ended,
            Err(Stuck) => {
                guest_init::say_out(&format!(
                    "FAILED: {name}, its processes still running {REAP:?} after they were killed"
                ));
                guest_init::crash();
            }
        }
    }

    /// Puts back what the check `name` left of the guest, and says what.
    fn put_back(&self, name: &str) {
        for (path, booted) in &self.params {
            let left = read_param(path);
            if left != *booted {
                fs::write(path, booted).unwrap();
                let param = path.file_name().unwrap().to_string_lossy();
                eprintln!("{name}: left fxfs.{param} at {left}, set back to {booted}");
            }
        }
        if thaw() {
            eprintln!("{name}: left the filesystem frozen, thawed");
        }
        if !mounted() {
            mount();
            eprintln!("{name}: left the image unmounted, mounted again");
        }
    }
}

fn read_param(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap().trim().to_owned()
}

/// Thaws the filesystem (FITHAW), if a check left it frozen. Whether it had.
fn thaw() -> bool {
    const FITHAW: libc::c_ulong = 0xc0045878;
    let dir = fs::File::open(MNT).unwrap();
    if unsafe { libc::ioctl(dir.as_raw_fd(), FITHAW as _, 0) } == 0 {
        return true;
    }
    let error = std::io::Error::last_os_error();
    // Not frozen; or not mounted, the mount point the root filesystem's, which has no freezing.
    let not_frozen = [libc::EINVAL, libc::EOPNOTSUPP];
    assert!(error.raw_os_error().is_some_and(|errno| not_frozen.contains(&errno)), "{error}");
    false
}

/// Whether the image is mounted.
fn mounted() -> bool {
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap();
    mounts.lines().any(|mount| mount.split_whitespace().nth(1) == Some(MNT))
}

/// fxdax: a check's time, the guest's: the time since it began, but for what the guest waited
/// meanwhile for the host, the device having a request in flight (the driver's `busy_ns`). The
/// host's time differs from host to host, and with their load: its disk's, making the guest's
/// commits durable, 4 times as long on some GCE hosts as on others; and its writes', which took
/// xattrs_bounded's fill from 0.6 s to 5.8 s on a host whose CPUs 8 agents shared (ubu1, load 40
/// to 150). What a check's budget bounds is its work, the guest's, and its waits on anything but
/// the device: a hang is still killed at its budget. The host's time counts too, but a
/// [`HOST_SCALE`]th of it, so that a device that never answers has the check killed, by name, as
/// any hang; what a check asks of the host, a check counts, as xattrs_bounded does. Budgets were of a store in the
/// daemon's memory (TASK-168), whose requests took little; since stores keep their extents in
/// files on the host's disk, the checks that fill the filesystem and sync waited seconds for it,
/// and overran (TASK-263).
struct Clock {
    began: Instant,
    /// The host's time since it began.
    host: Duration,
    /// The device's `busy_ns` as last read, if the driver was bound.
    last: Option<Duration>,
}

impl Clock {
    fn start() -> Self {
        Self { began: Instant::now(), host: Duration::ZERO, last: crate::extent_driver::busy() }
    }

    /// Its time against a budget ([`charged`]).
    fn spent(&mut self) -> Duration {
        let guest = self.guest();
        charged(guest, self.host)
    }

    /// The guest's time since it began.
    fn guest(&mut self) -> Duration {
        let now = crate::extent_driver::busy();
        // A device bound again counts from 0: what it counted since, all of it the check's.
        let grew = match (self.last, now) {
            (_, None) => Duration::ZERO,
            (Some(last), Some(now)) if now >= last => now - last,
            (_, Some(now)) => now,
        };
        (self.host, self.last) = (self.host + grew, now);
        self.began.elapsed().saturating_sub(self.host)
    }
}

/// fxdax: what a check's time is against its budget, of `guest`'s time and `host`'s: the guest's,
/// or a [`HOST_SCALE`]th of the host's, whichever is more ([`Clock`]).
fn charged(guest: Duration, host: Duration) -> Duration {
    guest.max(host.div_f64(HOST_SCALE))
}

/// fxdax: what of the guest's time the host took, so far: the time its vCPUs waited for the host's
/// CPUs (steal), and the device's requests, by type, how many and how long they took, all told
/// (the driver's `requests` and `request_ns`), which overlap. Of a check, the difference of two:
/// where its time went besides the guest (TASK-263).
struct HostTook {
    stolen: Duration,
    requests: std::collections::BTreeMap<String, (u64, Duration)>,
}

impl HostTook {
    fn now() -> Self {
        let stat = fs::read_to_string("/proc/stat").unwrap();
        // cpu user nice system idle iowait irq softirq steal: in USER_HZ ticks.
        let steal = stat.lines().next().unwrap().split_whitespace().nth(8).unwrap();
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
        let stolen = Duration::from_nanos(steal.parse::<u64>().unwrap() * 1_000_000_000 / hz);
        let counts = |name: &str| -> std::collections::BTreeMap<String, u64> {
            let text = crate::extent_driver::attribute(name).unwrap_or_default();
            let pairs = text.lines().filter_map(|line| line.split_once(' '));
            pairs.map(|(name, count)| (name.to_owned(), count.parse().unwrap())).collect()
        };
        let took = counts("request_ns");
        let requests = (counts("requests").into_iter())
            .map(|(name, n)| {
                let ns = took.get(&name).copied().unwrap_or(0);
                (name, (n, Duration::from_nanos(ns)))
            })
            .collect();
        Self { stolen, requests }
    }

    /// What the host took since `self`, as the check's line says it.
    fn since(&self) -> String {
        let now = Self::now();
        let stolen = now.stolen.saturating_sub(self.stolen);
        let mut said = format!("{stolen:.2?} of the guest's CPUs");
        for (name, (n, took)) in &now.requests {
            let (before, before_took) = self.requests.get(name).copied().unwrap_or_default();
            if *n > before {
                let took = took.saturating_sub(before_took);
                said += &format!(", {} {name} requests in {took:.2?}", n - before);
            }
        }
        said
    }
}

/// A check whose processes, killed, didn't end: the guest is too stuck to go on.
#[derive(Debug, PartialEq)]
struct Stuck;

/// Runs `command` in a process group of its own till it ends, or till `budget` of the time `spent`
/// says is up, from 0 as it starts: then has `overran` say so, and kills it. Then kills what is
/// left of the group, has `put_back` put back what it left of the guest, and waits for the group
/// to end, [`REAP`] at most: in that order, so that a task of it waiting in the kernel for what it
/// left, as on a frozen filesystem, goes on, and ends.
fn run_to(
    mut command: Command,
    budget: Duration,
    mut spent: impl FnMut() -> Duration,
    overran: impl FnOnce(),
    put_back: impl FnOnce(),
) -> Result<Ended, Stuck> {
    let mut child = command.process_group(0).spawn().expect("running a check");
    let group = child.id() as libc::pid_t;
    let (exited, exits) = mpsc::channel();
    std::thread::spawn(move || exited.send(child.wait().expect("waiting for a check")));
    // fxdax: till `budget` is spent, which a wall clock's time may not have spent (TASK-263).
    let ended = loop {
        let left = budget.saturating_sub(spent());
        if left.is_zero() {
            break None;
        }
        match exits.recv_timeout(left) {
            Ok(status) => break Some(Ended::of(status)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break None,
        }
    };
    if ended.is_none() {
        overran();
    }
    // SAFETY: A plain system call. The group ended already, if it has no process (ESRCH).
    unsafe { libc::kill(-group, libc::SIGKILL) };
    put_back();
    let deadline = Instant::now() + REAP;
    let ended = match ended {
        Some(ended) => ended,
        None => {
            let left = deadline.saturating_duration_since(Instant::now());
            exits.recv_timeout(left).map_err(|_| Stuck)?;
            Ended::Overran
        }
    };
    reap(group, deadline)?;
    Ok(ended)
}

/// Waits for what is left of the process group `group`, its first process gone, till
/// `deadline`: the processes it forked, the agent's as they are orphaned.
fn reap(group: libc::pid_t, deadline: Instant) -> Result<(), Stuck> {
    loop {
        // SAFETY: A plain system call.
        let waited = unsafe { libc::waitpid(-group, std::ptr::null_mut(), libc::WNOHANG) };
        if waited > 0 {
            continue;
        }
        if waited < 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(error.raw_os_error(), Some(libc::ECHILD), "waitpid: {error}");
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Stuck);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Ended {
    /// How a check's process ended, as its status says.
    fn of(status: ExitStatus) -> Self {
        match (status.code(), status.signal()) {
            (Some(0), _) => Ended::Passed,
            (Some(code), _) => Ended::Exited(code),
            (None, Some(signal)) => Ended::Signalled(signal),
            (None, None) => unreachable!("{status:?}: neither exited nor signalled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// `script`, run by the shell, as a check is: its end, whether it overran, and whether its
    /// guest's state was put back.
    fn run_script(script: &str, budget: Duration) -> (Result<Ended, Stuck>, bool, bool) {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        let (overran, put_back) = (Cell::new(false), Cell::new(false));
        let began = Instant::now();
        let spent = || began.elapsed();
        let ended = run_to(command, budget, spent, || overran.set(true), || put_back.set(true));
        (ended, overran.get(), put_back.get())
    }

    #[test]
    fn passes_and_fails_by_status() {
        let budget = Duration::from_secs(10);
        assert_eq!(run_script("exit 0", budget), (Ok(Ended::Passed), false, true));
        assert_eq!(run_script("exit 101", budget), (Ok(Ended::Exited(101)), false, true));
    }

    /// A crash fails the check by its signal. Without a core, which the GCE hosts' runs report as
    /// a process crashed (r610).
    #[test]
    fn crashes_fail_by_signal() {
        let ended = run_script("ulimit -c 0; kill -BUS $$", Duration::from_secs(10));
        assert_eq!(ended, (Ok(Ended::Signalled(libc::SIGBUS)), false, true));
    }

    /// A check running on past its budget is told of, killed, and fails.
    #[test]
    fn overruns_are_killed() {
        let began = Instant::now();
        let ended = run_script("while :; do :; done", Duration::from_millis(200));
        assert_eq!(ended, (Ok(Ended::Overran), true, true));
        assert!(began.elapsed() < REAP, "took {:?}", began.elapsed());
    }

    /// fxdax: time `spent` leaves out isn't a check's: one that runs past its budget, all of it
    /// time left out, runs on, and ends as it would have, before its budget of time not left out
    /// is up (TASK-263); and one whose time left out stops growing is killed at its budget.
    #[test]
    fn time_left_out_is_not_spent() {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 0.5");
        let began = Instant::now();
        // The first 0.6 s left out, as a durable commit's in flight are.
        let spent = || began.elapsed().saturating_sub(Duration::from_millis(600));
        let ended = run_to(command, Duration::from_millis(200), spent, || {}, || {});
        assert_eq!(ended, Ok(Ended::Passed));

        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 10");
        let began = Instant::now();
        let spent = || began.elapsed().saturating_sub(Duration::from_millis(300));
        let ended = run_to(command, Duration::from_millis(200), spent, || {}, || {});
        assert_eq!(ended, Ok(Ended::Overran));
        let took = began.elapsed();
        assert!(took >= Duration::from_millis(500) && took < REAP, "took {took:?}");
    }

    /// fxdax: the host's time counts a [`HOST_SCALE`]th: a check waiting on a device that never
    /// answers is killed, as a hang (TASK-263).
    #[test]
    fn the_hosts_time_counts_a_fraction() {
        let second = Duration::from_secs(1);
        assert_eq!(charged(2 * second, 18 * second), 2 * second);
        assert_eq!(charged(second / 2, 36 * second), 2 * second);
        assert_eq!(charged(Duration::ZERO, Duration::ZERO), Duration::ZERO);
    }

    /// What a check forked, and left running as it ended, is killed and waited for, its orphans
    /// the runner's, as the agent makes them (Guest::set_up).
    #[test]
    fn leftovers_are_killed() {
        assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
        let pid_file = std::env::temp_dir().join(format!("leftover-{}", std::process::id()));
        let script = format!("sh -c 'while :; do :; done' & echo $! > {}", pid_file.display());
        let ended = run_script(&script, Duration::from_secs(10));
        assert_eq!(ended, (Ok(Ended::Passed), false, true));
        let leftover: libc::pid_t = fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        fs::remove_file(&pid_file).unwrap();
        let error = (unsafe { libc::kill(leftover, 0) } == -1).then(std::io::Error::last_os_error);
        assert_eq!(error.and_then(|error| error.raw_os_error()), Some(libc::ESRCH));
    }
}
