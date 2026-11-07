// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! I/O on an image's files, run off the threads of the futures awaiting it.
//!
//! Fxfs's device I/O is awaited on its executor's threads, which must not block in `pread` and
//! `pwrite`. On Linux, the engine submits operations to an io_uring, whose completions a thread
//! of its own collects. Elsewhere, a small pool of threads runs them.
//!
//! An operation owns what the host touches, its buffer and its file, until the host is done with
//! them, and gives the buffer back as it completes. A future dropped, or leaked, while its
//! operation is in flight leaves them to the operation, which frees them once it completes: the
//! caller's memory is never the host's. It was, the future waiting for the operation as it was
//! dropped, but a future leaked (`mem::forget`) waited for nothing, which safe code may do of a
//! device's read, and the host went on writing memory its caller had back (TASK-24).

use extent_store::Aligned;
use futures::task::AtomicWaker;
use std::future::Future;
use std::io;
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

/// An operation on a file: what it owns of what the host touches.
pub(super) enum Op {
    /// Reads `len` bytes at `offset`, into a buffer of its own.
    Read { len: usize, offset: u64 },
    /// Writes `data` at `offset`.
    Write { data: Aligned, offset: u64 },
}

/// A file an operation runs on, which it keeps open until the host is done with it.
pub(super) type IoFile = Arc<dyn AsRawFd + Send + Sync>;

/// An operation as an engine runs it: on memory at `addr`, which the operation's completion owns.
#[derive(Clone, Copy, Debug)]
struct Raw {
    write: bool,
    addr: usize,
    len: usize,
    offset: u64,
}

/// What an operation did: the bytes it transferred, and its buffer, a read's bytes or a write's
/// data.
pub(super) struct Done {
    pub result: io::Result<usize>,
    pub buffer: Aligned,
}

/// The engine running operations on files.
pub(super) struct Io {
    engine: Arc<Engine>,
    /// A test's hold of the next operation: submitted once the test lets it go.
    #[cfg(test)]
    hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl Io {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            engine: Arc::new(Engine::new()?),
            #[cfg(test)]
            hold: Mutex::new(None),
        })
    }

    /// Runs `op` on `file`, and returns what it did.
    pub fn run(&self, file: IoFile, op: Op) -> InFlight {
        let (write, mut buffer, offset) = match op {
            Op::Read { len, offset } => (false, Aligned::new(len), offset),
            Op::Write { data, offset } => (true, data, offset),
        };
        let bytes = buffer.as_mut_slice();
        // The buffer's bytes are on the heap, where moving it into the completion leaves them.
        let raw = Raw { write, addr: bytes.as_mut_ptr() as usize, len: bytes.len(), offset };
        let fd = file.as_raw_fd();
        let completion = Arc::new(Completion {
            state: Mutex::new(State::InFlight { buffer, _file: file }),
            waker: AtomicWaker::new(),
        });
        #[cfg(test)]
        if let Some(hold) = self.hold.lock().unwrap().take() {
            let (engine, submitted) = (self.engine.clone(), completion.clone());
            std::thread::spawn(move || {
                let _ = hold.recv();
                engine.submit(fd, raw, submitted);
            });
            return InFlight { completion };
        }
        self.engine.submit(fd, raw, completion.clone());
        InFlight { completion }
    }
}

/// An operation's state: in flight, owning its buffer and its file, or done, with its result, a
/// negated errno on failure, and its buffer, until the future takes them.
enum State {
    InFlight { buffer: Aligned, _file: IoFile },
    Done { result: i64, buffer: Aligned },
    Taken,
}

/// An operation's state, which the engine holds until it completes, and its future's waker.
pub(super) struct Completion {
    state: Mutex<State>,
    waker: AtomicWaker,
}

impl Completion {
    /// The host is done with the operation's memory and file: the file is let go of, and the
    /// buffer kept for the future, or freed with the completion if the future is gone.
    fn complete(&self, result: i64) {
        let file = {
            let mut state = self.state.lock().unwrap();
            let State::InFlight { buffer, _file } = std::mem::replace(&mut *state, State::Taken)
            else {
                unreachable!("an operation completed twice");
            };
            *state = State::Done { result, buffer };
            _file
        };
        drop(file);
        self.waker.wake();
    }
}

/// An operation in flight, which completes with what it did. Dropped, or leaked, it leaves the
/// operation what it owns, which goes with the operation's completion.
pub(super) struct InFlight {
    completion: Arc<Completion>,
}

impl Future for InFlight {
    type Output = Done;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Done> {
        self.completion.waker.register(cx.waker());
        let mut state = self.completion.state.lock().unwrap();
        if matches!(*state, State::InFlight { .. }) {
            return Poll::Pending;
        }
        let State::Done { result, buffer } = std::mem::replace(&mut *state, State::Taken) else {
            panic!("an operation's future polled after it completed");
        };
        let result = if result < 0 {
            Err(io::Error::from_raw_os_error(-result as i32))
        } else {
            Ok(result as usize)
        };
        Poll::Ready(Done { result, buffer })
    }
}

#[cfg(target_os = "linux")]
use uring::Engine;

#[cfg(not(target_os = "linux"))]
use pool::Engine;

/// The io_uring engine.
#[cfg(target_os = "linux")]
mod uring {
    use super::{Completion, Raw};
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

        pub fn submit(&self, fd: RawFd, raw: Raw, completion: Arc<Completion>) {
            let fd = types::Fd(fd);
            let Raw { write, addr, len, offset } = raw;
            let entry = match write {
                false => opcode::Read::new(fd, addr as *mut u8, len as u32).offset(offset).build(),
                true => {
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
            // entry's buffer and file are its completion's, which `in_flight` holds until the
            // kernel completes it.
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
    use super::{Completion, Raw};
    use std::io;
    use std::os::fd::RawFd;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    const THREADS: usize = 4;

    type Job = (RawFd, Raw, Arc<Completion>);

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
                            let Ok((fd, raw, completion)) = job else {
                                break;
                            };
                            completion.complete(run(fd, raw));
                        }
                    })
                })
                .collect::<io::Result<_>>()?;
            Ok(Self { jobs: Some(jobs), threads })
        }

        pub fn submit(&self, fd: RawFd, raw: Raw, completion: Arc<Completion>) {
            self.jobs
                .as_ref()
                .unwrap()
                .send((fd, raw, completion))
                .expect("the io threads stopped");
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

    /// Runs `raw` on `fd`, returning its result, or a negated errno.
    fn run(fd: RawFd, raw: Raw) -> i64 {
        let Raw { write, addr, len, offset } = raw;
        // SAFETY: `fd` is open and the buffer valid: both are the operation's completion's, which
        // the job holds until this returns.
        let result = unsafe {
            match write {
                false => libc::pread(fd, addr as *mut _, len, offset as libc::off_t) as i64,
                true => libc::pwrite(fd, addr as *const _, len, offset as libc::off_t) as i64,
            }
        };
        if result < 0 {
            -(io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO) as i64)
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A read whose future is leaked, safe code being free to, writes none of its caller's memory,
    /// and frees its own once it completes: before TASK-24, the caller's buffer was the host's
    /// until the future completed or was dropped, and a leaked one was neither, which the host
    /// went on writing as its caller reused it.
    #[test]
    fn a_leaked_read_writes_only_what_it_owns() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&[7u8; 4096]).unwrap();
        let file = Arc::new(file);
        let io = Io::new().unwrap();
        let (release, hold) = std::sync::mpsc::channel();
        *io.hold.lock().unwrap() = Some(hold);
        let in_flight = io.run(file.clone(), Op::Read { len: 4096, offset: 0 });
        let completion = Arc::downgrade(&in_flight.completion);
        std::mem::forget(in_flight);
        assert_eq!(Arc::strong_count(&file), 2, "the operation keeps the file open");
        release.send(()).unwrap();
        // Waiting for it as the leaked future would have: it read into its own buffer, and,
        // done, let the file go.
        let completion = completion.upgrade().expect("held by the leaked future");
        let Done { result, buffer } = futures::executor::block_on(InFlight { completion });
        assert_eq!(result.unwrap(), 4096);
        assert!(buffer.as_slice().iter().all(|&b| b == 7));
        assert_eq!(Arc::strong_count(&file), 1, "the operation let the file go");
    }

    /// Reads and writes give their buffers back with what they did.
    #[test]
    fn operations_give_their_buffers_back() {
        let file = Arc::new(tempfile::tempfile().unwrap());
        let io = Io::new().unwrap();
        let mut data = Aligned::new(4096);
        data.as_mut_slice().fill(3);
        let Done { result, buffer } =
            futures::executor::block_on(io.run(file.clone(), Op::Write { data, offset: 4096 }));
        assert_eq!(result.unwrap(), 4096);
        assert!(buffer.as_slice().iter().all(|&b| b == 3));
        let Done { result, buffer } =
            futures::executor::block_on(io.run(file, Op::Read { len: 8192, offset: 0 }));
        assert_eq!(result.unwrap(), 8192);
        assert!(buffer.as_slice()[..4096].iter().all(|&b| b == 0));
        assert!(buffer.as_slice()[4096..].iter().all(|&b| b == 3));
    }
}
