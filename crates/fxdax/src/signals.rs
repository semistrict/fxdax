// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: signals a process takes on a thread of its own, which waits for them (`sigwait`), rather
//! than in a handler, which may do next to nothing: the extent daemon's SIGTERM, which drains it
//! (TASK-141.4), any process's SIGQUIT, which logs its threads' stacks (`crate::stacks`,
//! TASK-44.13), and `fxdax-mount`'s SIGINT and SIGTERM, which unmount it (TASK-222).
//!
//! A signal a thread doesn't block may be delivered to it, which ends the process by the signal's
//! default action, however another thread waits for it. So [`Signals::block`] blocks its signals in
//! the calling thread, which the threads it starts from then on inherit: a process calls it before
//! it starts any. The thread that waits for them is started by [`spawn_taking_no_signals`].

use std::io;

/// Signals the process blocks, for a thread to wait for.
pub struct Signals(libc::sigset_t);

impl Signals {
    /// Blocks `signals` in the calling thread, and in those it starts from now on. Call it before
    /// the process starts other threads, which would take the signals otherwise.
    pub fn block(signals: &[libc::c_int]) -> io::Result<Self> {
        // SAFETY: A signal set, filled by the calls below.
        let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: Plain calls on a signal set this function owns, and on the calling thread's
        // mask.
        unsafe {
            libc::sigemptyset(&mut set);
            for &signal in signals {
                if libc::sigaddset(&mut set, signal) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            let status = libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            if status != 0 {
                return Err(io::Error::from_raw_os_error(status));
            }
        }
        Ok(Self(set))
    }

    /// Waits for one of the signals, and returns it.
    pub fn wait(&self) -> io::Result<libc::c_int> {
        let mut signal = 0;
        // SAFETY: Waits for a signal of the set, which every thread blocks.
        match unsafe { libc::sigwait(&self.0, &mut signal) } {
            0 => Ok(signal),
            status => Err(io::Error::from_raw_os_error(status)),
        }
    }
}

/// Starts a thread that takes no signal: blocked from before it starts, as a thread blocking them
/// itself would take one until it first ran, however late that is, and a process that leaves the
/// signal to another thread, as extentd does SIGTERM, would die of it.
pub fn spawn_taking_no_signals(name: &str, work: impl FnOnce() + Send + 'static) -> io::Result<()> {
    // SAFETY: Signal sets, filled by the calls below.
    let (mut all, mut mask): (libc::sigset_t, libc::sigset_t) =
        unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    // SAFETY: Plain calls on signal sets this function owns, and on the calling thread's mask,
    // which it puts back below.
    unsafe {
        libc::sigfillset(&mut all);
        let status = libc::pthread_sigmask(libc::SIG_SETMASK, &all, &mut mask);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status));
        }
    }
    let spawned = std::thread::Builder::new().name(name.into()).spawn(work);
    // SAFETY: As above.
    let status = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut()) };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status));
    }
    spawned.map(drop)
}

#[cfg(test)]
mod tests {
    /// Whether the calling thread blocks SIGTERM.
    fn blocks_sigterm() -> bool {
        // SAFETY: A signal set, filled by the call below.
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        // SAFETY: Reads the calling thread's mask into a signal set of this function's.
        assert_eq!(unsafe { libc::pthread_sigmask(0, std::ptr::null(), &mut mask) }, 0);
        // SAFETY: A signal set the call above filled.
        unsafe { libc::sigismember(&mask, libc::SIGTERM) == 1 }
    }

    /// The first thing such a thread does finds every signal blocked, so no signal can reach it
    /// however late it first runs: `the_daemon_stops_on_sigterm` died of one under load, a thread
    /// that blocked them itself not having run yet. Its caller's mask stays as it was.
    #[test]
    fn a_thread_taking_no_signals_takes_none_from_its_first_instruction() {
        let before = blocks_sigterm();
        let (sender, blocked) = std::sync::mpsc::channel();
        super::spawn_taking_no_signals("test", move || sender.send(blocks_sigterm()).unwrap())
            .unwrap();
        assert!(blocked.recv().unwrap());
        assert_eq!(blocks_sigterm(), before);
    }
}
