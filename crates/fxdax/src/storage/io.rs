// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! I/O on an image's files, run off the threads of the futures awaiting it.
//!
//! Fxfs's device I/O is awaited on its executor's threads, which must not block in `pread` and
//! `pwrite`. On Linux, the engine submits operations to an io_uring, whose completions a thread
//! of its own collects. Elsewhere, a small pool of threads runs them.
//!
//! An operation's buffer is the caller's until the host is done with it, so a future dropped while
//! its operation is in flight waits for the operation to complete before it returns.

use futures::task::AtomicWaker;
use std::future::Future;
use std::io;
use std::os::fd::RawFd;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll};

/// An operation on a file.
#[derive(Clone, Copy, Debug)]
pub(super) enum Op {
    /// Reads `len` bytes at `offset` into memory at `addr`.
    Read { addr: usize, len: usize, offset: u64 },
    /// Writes `len` bytes at `offset` from memory at `addr`.
    Write { addr: usize, len: usize, offset: u64 },
}

/// The engine running operations on files.
pub(super) struct Io {
    engine: Engine,
}

impl Io {
    pub fn new() -> io::Result<Self> {
        Ok(Self { engine: Engine::new()? })
    }

    /// Runs `op` on `fd`, and returns its result: the bytes transferred.
    ///
    /// # Safety
    ///
    /// `fd` must stay open, and a read's or write's memory valid for its access, until the
    /// returned future completes or is dropped. The future must not be leaked (`mem::forget`):
    /// only completing or dropping it waits for the host to be done with the memory.
    pub unsafe fn run(&self, fd: RawFd, op: Op) -> InFlight {
        let completion = Arc::new(Completion::default());
        self.engine.submit(fd, op, completion.clone());
        InFlight { completion, done: false }
    }
}

/// The result of an operation once it completes, and its waiters.
#[derive(Default)]
pub(super) struct Completion {
    /// The operation's result, as a negated errno on failure.
    result: Mutex<Option<i64>>,
    /// Notified when the result arrives, for a dropped future waiting for it.
    done: Condvar,
    waker: AtomicWaker,
}

impl Completion {
    fn complete(&self, result: i64) {
        *self.result.lock().unwrap() = Some(result);
        self.done.notify_all();
        self.waker.wake();
    }

    fn result(&self) -> Option<i64> {
        *self.result.lock().unwrap()
    }
}

/// An operation in flight, which completes with its result.
pub(super) struct InFlight {
    completion: Arc<Completion>,
    done: bool,
}

impl Future for InFlight {
    type Output = io::Result<usize>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        self.completion.waker.register(cx.waker());
        let Some(result) = self.completion.result() else {
            return Poll::Pending;
        };
        self.done = true;
        Poll::Ready(if result < 0 {
            Err(io::Error::from_raw_os_error(-result as i32))
        } else {
            Ok(result as usize)
        })
    }
}

impl Drop for InFlight {
    /// Waits for the operation, which may still be using its buffer, to complete.
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let mut result = self.completion.result.lock().unwrap();
        while result.is_none() {
            result = self.completion.done.wait(result).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
use uring::Engine;

#[cfg(not(target_os = "linux"))]
use pool::Engine;

/// The io_uring engine.
#[cfg(target_os = "linux")]
mod uring {
    use super::{Completion, Op};
    use io_uring::{IoUring, opcode, squeue, types};
    use std::collections::HashMap;
    use std::io;
    use std::os::fd::RawFd;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    const ENTRIES: u32 = 256;

    /// The user data of the operation that stops the completion thread.
    const STOP: u64 = u64::MAX;

    pub(in super::super) struct Engine {
        ring: Arc<IoUring>,
        /// The next operation's user data, locked to serialize submissions.
        submissions: Mutex<u64>,
        /// The operations in flight, by their user data.
        in_flight: Arc<Mutex<HashMap<u64, Arc<Completion>>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Engine {
        pub fn new() -> io::Result<Self> {
            let ring = Arc::new(IoUring::new(ENTRIES)?);
            let in_flight = Arc::new(Mutex::new(HashMap::new()));
            let thread = std::thread::Builder::new().name("fxdax io".into()).spawn({
                let (ring, in_flight) = (ring.clone(), in_flight.clone());
                move || complete(&ring, &in_flight)
            })?;
            Ok(Self { ring, submissions: Mutex::new(0), in_flight, thread: Some(thread) })
        }

        pub fn submit(&self, fd: RawFd, op: Op, completion: Arc<Completion>) {
            let fd = types::Fd(fd);
            let entry = match op {
                Op::Read { addr, len, offset } => {
                    opcode::Read::new(fd, addr as *mut u8, len as u32).offset(offset).build()
                }
                Op::Write { addr, len, offset } => {
                    opcode::Write::new(fd, addr as *const u8, len as u32).offset(offset).build()
                }
            };
            let mut next = self.submissions.lock().unwrap();
            let user_data = *next;
            *next += 1;
            // In flight before it is queued, as it may complete as soon as it is.
            self.in_flight.lock().unwrap().insert(user_data, completion);
            if let Err(err) = push(&self.ring, &entry.user_data(user_data)) {
                let completion = self.in_flight.lock().unwrap().remove(&user_data).unwrap();
                completion.complete(-(err.raw_os_error().unwrap_or(libc::EIO) as i64));
            }
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            {
                let _submissions = self.submissions.lock().unwrap();
                push(&self.ring, &opcode::Nop::new().build().user_data(STOP))
                    .expect("queue the io thread's stop");
            }
            self.thread.take().unwrap().join().expect("the io thread panicked");
        }
    }

    /// Queues `entry` and hands it to the kernel, which the caller serializes with other
    /// submissions. Fails only if `entry` could not be queued: once it is, it completes, as the
    /// completion thread hands the kernel what is queued if this couldn't.
    fn push(ring: &IoUring, entry: &squeue::Entry) -> io::Result<()> {
        loop {
            // SAFETY: Submissions are serialized, so this is the queue's only producer, and the
            // entry's buffers are valid until it completes, as `Io::run`'s caller guarantees.
            let pushed = unsafe { ring.submission_shared().push(entry) };
            if pushed.is_ok() {
                break;
            }
            // The queue is full: hand what is queued to the kernel, and try again.
            submit(ring)?;
        }
        if let Err(err) = submit(ring) {
            log::error!("fxdax: submitting image I/O to io_uring failed: {err}");
        }
        Ok(())
    }

    /// Hands the queued entries to the kernel. While the completion queue is full (`EBUSY`) or
    /// the kernel is short of memory (`EAGAIN`), it retries once the completion thread has had a
    /// chance to take completions.
    fn submit(ring: &IoUring) -> io::Result<()> {
        loop {
            match ring.submit() {
                Ok(_) => return Ok(()),
                Err(err) if err.raw_os_error() == Some(libc::EINTR) => {}
                Err(err) if matches!(err.raw_os_error(), Some(libc::EBUSY | libc::EAGAIN)) => {
                    std::thread::yield_now();
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Completes the operations in flight as the ring reports them, until it reports `STOP` and
    /// none is left in flight.
    fn complete(ring: &IoUring, in_flight: &Mutex<HashMap<u64, Arc<Completion>>>) {
        let mut stopping = false;
        loop {
            if stopping && in_flight.lock().unwrap().is_empty() {
                return;
            }
            match ring.submit_and_wait(1) {
                Ok(_) => {}
                Err(err) if err.raw_os_error() == Some(libc::EINTR) => continue,
                // The completion queue is full: take completions below, and wait again.
                Err(err) if err.raw_os_error() == Some(libc::EBUSY) => {}
                Err(err) => panic!("wait for io_uring completions: {err}"),
            }
            // SAFETY: This thread is the completion queue's only consumer.
            for entry in unsafe { ring.completion_shared() } {
                if entry.user_data() == STOP {
                    stopping = true;
                    continue;
                }
                let completion = in_flight.lock().unwrap().remove(&entry.user_data());
                completion.expect("an operation in flight").complete(entry.result().into());
            }
        }
    }
}

/// The thread pool engine.
#[cfg(not(target_os = "linux"))]
mod pool {
    use super::{Completion, Op};
    use std::io;
    use std::os::fd::RawFd;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    const THREADS: usize = 4;

    type Job = (RawFd, Op, Arc<Completion>);

    pub(in super::super) struct Engine {
        jobs: Option<mpsc::Sender<Job>>,
        threads: Vec<JoinHandle<()>>,
    }

    impl Engine {
        pub fn new() -> io::Result<Self> {
            let (jobs, receiver) = mpsc::channel::<Job>();
            let receiver = Arc::new(Mutex::new(receiver));
            let threads = (0..THREADS)
                .map(|_| {
                    let receiver = receiver.clone();
                    std::thread::Builder::new().name("fxdax io".into()).spawn(move || {
                        loop {
                            // The receiver is unlocked while the operation runs.
                            let job = receiver.lock().unwrap().recv();
                            let Ok((fd, op, completion)) = job else {
                                break;
                            };
                            completion.complete(run(fd, op));
                        }
                    })
                })
                .collect::<io::Result<_>>()?;
            Ok(Self { jobs: Some(jobs), threads })
        }

        pub fn submit(&self, fd: RawFd, op: Op, completion: Arc<Completion>) {
            self.jobs.as_ref().unwrap().send((fd, op, completion)).expect("the io threads stopped");
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            drop(self.jobs.take());
            for thread in self.threads.drain(..) {
                thread.join().expect("an io thread panicked");
            }
        }
    }

    /// Runs `op` on `fd`, returning its result, or a negated errno.
    fn run(fd: RawFd, op: Op) -> i64 {
        // SAFETY: `Io::run`'s caller guarantees that `fd` is open and the buffers valid.
        let result = unsafe {
            match op {
                Op::Read { addr, len, offset } => {
                    libc::pread(fd, addr as *mut _, len, offset as libc::off_t) as i64
                }
                Op::Write { addr, len, offset } => {
                    libc::pwrite(fd, addr as *const _, len, offset as libc::off_t) as i64
                }
            }
        };
        if result < 0 {
            -(io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO) as i64)
        } else {
            result
        }
    }
}
