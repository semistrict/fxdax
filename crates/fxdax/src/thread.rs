// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The threads fxdax starts in a VMM's process, for a VM's store and DAX window, and those the VMM
//! starts for its device (TASK-50.1). The VMM starts them through its [`Spawn`], from a thread
//! confined as it wants them, as one under Firecracker's VMM seccomp filter is, rather than from
//! whichever thread asks: a vCPU's, activating the device, whose confinement they would inherit
//! for good, as a thread can only add to its seccomp filters. Without one, they start where they
//! are asked.

use std::sync::{OnceLock, mpsc};

/// What starts threads for fxdax: from a thread of its own, whose confinement they inherit.
pub trait Spawn: Send + Sync {
    /// Starts a thread named `name`, running `run`.
    fn spawn(&self, name: String, run: Box<dyn FnOnce() + Send>) -> std::io::Result<()>;
}

static SPAWNER: OnceLock<Box<dyn Spawn>> = OnceLock::new();

/// Has `spawner` start every thread [`spawn`] starts in this process from now on; once a process.
pub fn set_spawner(spawner: impl Spawn + 'static) {
    if SPAWNER.set(Box::new(spawner)).is_err() {
        panic!("fxdax's thread spawner is set once a process");
    }
}

/// A thread [`spawn`] started, which returns what it ran returns.
pub struct Thread<T> {
    result: mpsc::Receiver<T>,
}

impl<T> std::fmt::Debug for Thread<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Thread").finish_non_exhaustive()
    }
}

impl<T> Thread<T> {
    /// Waits for the thread to end, and returns what it returned: an error if it panicked.
    pub fn join(self) -> std::thread::Result<T> {
        self.result.recv().map_err(|_| Box::new("the thread panicked") as _)
    }
}

/// Starts the thread `name`, running `run`, through the [`Spawn`] set, if any.
pub fn spawn<T: Send + 'static>(
    name: impl Into<String>,
    run: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<Thread<T>> {
    let (done, result) = mpsc::sync_channel(1);
    let run = Box::new(move || {
        let _ = done.send(run());
    });
    let name = name.into();
    match SPAWNER.get() {
        Some(spawner) => spawner.spawn(name, run)?,
        None => drop(std::thread::Builder::new().name(name).spawn(run)?),
    }
    Ok(Thread { result })
}

/// Runs `run` on a thread [`spawn`] starts, `name`, and waits for it: for work that must not run
/// on the calling thread, under its confinement, as a vCPU's activating a device would. What `run`
/// borrows it borrows only until then, as with [`std::thread::scope`]; a panic of it is this
/// function's.
pub fn run<'a, T: Send + 'static>(
    name: impl Into<String>,
    run: impl FnOnce() -> T + Send + 'a,
) -> std::io::Result<T> {
    let run: Box<dyn FnOnce() -> T + Send + 'a> = Box::new(run);
    // SAFETY: Only the lifetime changes: this function returns only once `run` has returned or
    // unwound on the thread, its captures dropped (`spawn`'s thread sends what it returned after),
    // or, if the thread does not start, once `spawn` has dropped it.
    let run: Box<dyn FnOnce() -> T + Send + 'static> = unsafe { std::mem::transmute(run) };
    match spawn(name, run)?.join() {
        Ok(result) => Ok(result),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread returns what it ran returns; one that panicked, an error.
    #[test]
    fn threads_return_what_they_ran_returns() {
        assert_eq!(spawn("returns", || 42).unwrap().join().unwrap(), 42);
        let panicked = spawn("panics", || -> u8 { panic!("as it should") }).unwrap();
        assert!(panicked.join().is_err());
    }

    /// What runs on another thread borrows the caller's, which it changes before the caller goes
    /// on.
    #[test]
    fn what_runs_elsewhere_borrows_the_callers() {
        let mut counted = vec![1, 2];
        let sum = run("borrows", || {
            counted.push(3);
            counted.iter().sum::<i32>()
        })
        .unwrap();
        assert_eq!((sum, counted), (6, vec![1, 2, 3]));
    }
}
