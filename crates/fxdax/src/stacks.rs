// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: this process's threads' stacks, on standard error, when it is sent SIGQUIT (TASK-44.13):
//! what waits for what in a process that hangs, as `fxdaxctl import` did (TASK-98), which its
//! user then needed gdb on the host to see. elfutils' eu-stack reads them, from a child of this
//! process: Yama's ptrace_scope lets a process trace its descendants alone, so this one names the
//! child its tracer (`PR_SET_PTRACER`) before the child, waiting for that, becomes eu-stack.

use std::io::Write;
use std::process::{Command, Stdio};

/// Blocks SIGQUIT in the calling thread, and those it starts from now on, and starts a thread that
/// writes the process's stacks to standard error each time it is sent: before the process starts
/// other threads, which would take the signal otherwise, and end it.
pub fn on_sigquit() -> std::io::Result<()> {
    // SAFETY: A signal set, filled by the calls below.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: Plain calls on a signal set this function owns, and on the calling thread's mask.
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGQUIT);
        let status = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status));
        }
    }
    std::thread::Builder::new().name("stacks on SIGQUIT".into()).spawn(move || {
        // Every other signal for the threads that wait for it, as SIGTERM's does in extentd.
        // SAFETY: Plain calls on a signal set of this thread's, and on its mask.
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
        }
        loop {
            let mut signal = 0;
            // SAFETY: Waits for a signal of the set, which every thread blocks.
            let status = unsafe { libc::sigwait(&set, &mut signal) };
            if status != 0 {
                log::error!("waiting for SIGQUIT: {}", std::io::Error::from_raw_os_error(status));
                return;
            }
            log_stacks();
        }
    })?;
    Ok(())
}

/// Writes every thread's stack to standard error, through eu-stack, or why it couldn't.
pub fn log_stacks() {
    match stacks() {
        Ok(stacks) => eprintln!("fxdax: the threads' stacks, on SIGQUIT:\n{stacks}"),
        Err(err) => eprintln!("fxdax: the threads' stacks, on SIGQUIT: {err}"),
    }
}

fn stacks() -> std::io::Result<String> {
    let pid = std::process::id();
    // The child waits until it may trace this process, then is eu-stack, as the same process.
    let mut child = Command::new("sh")
        .args(["-c", &format!("read -r ready && exec eu-stack -p {pid}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // SAFETY: A plain prctl call naming this process's tracer.
    let named = unsafe { libc::prctl(libc::PR_SET_PTRACER, child.id() as libc::c_ulong, 0, 0, 0) };
    if named != 0 {
        log::warn!("naming eu-stack this process's tracer: {}", std::io::Error::last_os_error());
    }
    child.stdin.take().expect("its standard input").write_all(b"ready\n")?;
    let output = child.wait_with_output()?;
    // SAFETY: As above, naming no tracer again.
    unsafe { libc::prctl(libc::PR_SET_PTRACER, 0, 0, 0, 0) };
    let mut stacks = String::from_utf8_lossy(&output.stdout).into_owned();
    stacks.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(stacks)
}

#[cfg(test)]
mod tests {
    /// A process's stacks name the functions its threads wait in: here, a thread parked in one of
    /// this test's.
    /// eu-stack is elfutils', which the hosts the tests run on have (scripts/lib/linux-host-setup.sh,
    /// scripts/lima-setup.sh).
    #[test]
    fn stacks_name_where_threads_wait() {
        #[inline(never)]
        fn waits_for_ever(parked: std::sync::mpsc::Sender<()>) {
            parked.send(()).unwrap();
            std::thread::park();
        }
        let (parked, parking) = std::sync::mpsc::channel();
        std::thread::spawn(move || waits_for_ever(parked));
        parking.recv().unwrap();
        let stacks = super::stacks().unwrap();
        assert!(stacks.contains("waits_for_ever"), "{stacks}");
    }
}
