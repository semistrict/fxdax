// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: the agent's running of the checks, each in a process of its own: the agent run again,
//! the check named in its environment ([`ALONE`]), to a deadline, the check's budget (TASK-168).
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
use std::sync::mpsc;

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
            let began = Instant::now();
            let ended = guest.run(check.name, budget);
            // fxdax: each check's time, against its budget, in the run's log (TASK-168).
            let took = began.elapsed();
            if ended == Ended::Passed {
                eprintln!("PASS: {} ({took:.2?} of {budget:.2?})", check.name);
            } else {
                eprintln!("FAIL: {} ({took:.2?} of {budget:.2?}): {ended}", check.name);
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

    /// Runs the check `name` in a process of its own, the agent run again, to `budget`, past
    /// which the guest's stacks are said and the check killed; puts back what it left of the
    /// guest; and says how it ended. Resets the VM if its processes don't end once killed.
    fn run(&self, name: &str, budget: Duration) -> Ended {
        let mut check = Command::new("/proc/self/exe");
        check.args(std::env::args_os().skip(1)).env(ALONE, name);
        let overran = || {
            let stacks = crate::kernel_log::stacks().concat();
            guest_init::complain_out(&format!(
                "{name}: still running after its budget of {budget:.2?}; the guest's stacks:\n\
                 {stacks}"
            ));
        };
        match run_to(check, budget, overran, || self.put_back(name)) {
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

/// A check whose processes, killed, didn't end: the guest is too stuck to go on.
#[derive(Debug, PartialEq)]
struct Stuck;

/// Runs `command` in a process group of its own till it ends, or till `budget` is up: then has
/// `overran` say so, and kills it. Then kills what is left of the group, has `put_back` put back
/// what it left of the guest, and waits for the group to end, [`REAP`] at most: in that order, so
/// that a task of it waiting in the kernel for what it left, as on a frozen filesystem, goes on,
/// and ends.
fn run_to(
    mut command: Command,
    budget: Duration,
    overran: impl FnOnce(),
    put_back: impl FnOnce(),
) -> Result<Ended, Stuck> {
    let mut child = command.process_group(0).spawn().expect("running a check");
    let group = child.id() as libc::pid_t;
    let (exited, exits) = mpsc::channel();
    std::thread::spawn(move || exited.send(child.wait().expect("waiting for a check")));
    let ended = exits.recv_timeout(budget).ok().map(Ended::of);
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
        let ended = run_to(command, budget, || overran.set(true), || put_back.set(true));
        (ended, overran.get(), put_back.get())
    }

    #[test]
    fn passes_and_fails_by_status() {
        let budget = Duration::from_secs(10);
        assert_eq!(run_script("exit 0", budget), (Ok(Ended::Passed), false, true));
        assert_eq!(run_script("exit 101", budget), (Ok(Ended::Exited(101)), false, true));
    }

    /// A crash fails the check by its signal.
    #[test]
    fn crashes_fail_by_signal() {
        let ended = run_script("kill -BUS $$", Duration::from_secs(10));
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
