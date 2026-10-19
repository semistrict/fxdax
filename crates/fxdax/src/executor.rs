// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An executor of its own for Fxfs's futures, which callers await from their own executors, or
//! threads.

use futures::FutureExt;
use futures::channel::oneshot;
use std::cell::Cell;
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

/// Fxfs's executor: a fuchsia-async `SendExecutor` on threads of its own, which stops once it is
/// dropped and the futures it is running complete. Futures are spawned straight onto its runtime,
/// from whatever thread runs them, rather than handed to a task of its own to spawn.
pub(crate) struct Executor {
    /// The executor's runtime, which spawns futures from any thread.
    runtime: tokio::runtime::Handle,
    /// The futures running, which the executor finishes before it stops: its runtime would drop
    /// them unfinished.
    jobs: Arc<Jobs>,
    /// Dropped, stops the executor once its futures complete.
    _stop: oneshot::Sender<()>,
}

impl Executor {
    /// Starts an executor with `threads` threads.
    pub fn new(threads: u8) -> Self {
        let jobs = Arc::new(Jobs::default());
        let (stop, stopped) = oneshot::channel::<()>();
        let (runtime, started) = std::sync::mpsc::sync_channel(1);
        let running = jobs.clone();
        std::thread::Builder::new()
            .name("fxdax executor".into())
            .spawn(move || {
                fuchsia_async::SendExecutorBuilder::new().num_threads(threads).build().run(
                    async move {
                        let _ = runtime.send(tokio::runtime::Handle::current());
                        // The sender dropped stops it as well as a message would.
                        let _ = stopped.await;
                        running.idle().await;
                    },
                )
            })
            .expect("failed to start the executor's thread");
        let runtime = started.recv().expect("the executor's thread stopped starting");
        Self { runtime, jobs, _stop: stop }
    }

    /// Runs `future` on the executor's threads.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        let running = Running::new(self.jobs.clone());
        self.runtime.spawn(OnExecutor(future.map(move |()| drop(running))));
    }

    /// Runs `future` on the executor, and returns its output once it completes.
    ///
    /// The future runs to completion even if the returned future is dropped first, so that a
    /// caller giving up on an operation never leaves it half done.
    pub async fn run<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> T {
        let (sender, receiver) = oneshot::channel();
        self.spawn(async move {
            let _ = sender.send(future.await);
        });
        receiver.await.expect("the executor dropped a future")
    }

    /// Runs `future` on the executor, blocking the calling thread until it completes, for callers
    /// that cannot await it, such as a destructor. It waits on a channel rather than running an
    /// executor, so that it may be called from a task of another executor.
    pub fn run_blocking<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> T {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.spawn(async move {
            let _ = sender.send(future.await);
        });
        receiver.recv().expect("the executor dropped a future")
    }
}

/// The number of an executor's futures running, which its thread waits to drop to zero before it
/// stops.
#[derive(Default)]
struct Jobs {
    running: AtomicUsize,
    idle: tokio::sync::Notify,
}

impl Jobs {
    /// Waits until no future is running. None can start once the executor is dropped.
    async fn idle(&self) {
        loop {
            let mut idle = pin!(self.idle.notified());
            idle.as_mut().enable();
            if self.running.load(Ordering::Acquire) == 0 {
                return;
            }
            idle.await;
        }
    }
}

/// One of an executor's futures running, until it is dropped, whether the future completes or
/// panics.
struct Running(Arc<Jobs>);

impl Running {
    fn new(jobs: Arc<Jobs>) -> Self {
        jobs.running.fetch_add(1, Ordering::AcqRel);
        Self(jobs)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.0.running.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

thread_local! {
    /// Whether the thread is polling one of the executor's jobs.
    static ON_EXECUTOR: Cell<bool> = const { Cell::new(false) };
}

/// A job of the executor's, which marks its thread as the executor's while it is polled.
struct OnExecutor<F>(F);

impl<F: Future> Future for OnExecutor<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: The inner future is pinned as the wrapper is, and never moved out.
        let future = unsafe { self.map_unchecked_mut(|this| &mut this.0) };
        /// Restores the thread's mark, even if the poll panics.
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                ON_EXECUTOR.set(self.0);
            }
        }
        let _restore = Restore(ON_EXECUTOR.replace(true));
        future.poll(cx)
    }
}

/// Asserts that the calling thread isn't running one of the executor's jobs, whose futures a
/// blocking call there could wait for.
pub(crate) fn assert_not_on_executor() {
    assert!(!ON_EXECUTOR.get(), "blocking call on an executor thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Futures spawned run to their ends once the executor is dropped, and then it stops.
    #[test]
    fn futures_finish_after_the_executor_is_dropped_and_then_it_stops() {
        let executor = Executor::new(1);
        let runtime = executor.runtime.clone();
        let (go, wait) = oneshot::channel::<()>();
        let (done, finished) = std::sync::mpsc::channel();
        executor.spawn(async move {
            wait.await.unwrap();
            done.send(()).unwrap();
        });
        drop(executor);
        go.send(()).unwrap();
        assert_eq!(finished.recv_timeout(Duration::from_secs(30)), Ok(()));
        // Stopped, its runtime runs nothing more.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (ran, running) = std::sync::mpsc::channel();
            runtime.spawn(async move { ran.send(()).unwrap() });
            if running.recv_timeout(Duration::from_millis(100)).is_err() {
                break;
            }
            assert!(Instant::now() < deadline, "the executor runs on");
        }
    }

    /// The executor's thread waits, before it stops, until the last future running ends.
    #[test]
    fn idle_waits_for_the_last_future_running() {
        let jobs = Arc::new(Jobs::default());
        let (first, second) = (Running::new(jobs.clone()), Running::new(jobs.clone()));
        let mut idle = Box::pin(jobs.idle());
        let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        assert!(idle.as_mut().poll(&mut cx).is_pending());
        drop(first);
        assert!(idle.as_mut().poll(&mut cx).is_pending());
        drop(second);
        assert!(idle.as_mut().poll(&mut cx).is_ready());
    }

    /// Blocking calls fail in the executor's jobs, and only there: the mark goes with the job.
    #[test]
    fn jobs_are_marked_while_they_run() {
        let executor = Executor::new(1);
        let in_job = executor
            .run_blocking(async { std::panic::catch_unwind(assert_not_on_executor).is_err() });
        assert!(in_job);
        let (after, checked) = std::sync::mpsc::channel();
        executor.runtime.spawn(async move {
            after.send(std::panic::catch_unwind(assert_not_on_executor).is_ok()).unwrap();
        });
        assert_eq!(checked.recv_timeout(Duration::from_secs(30)), Ok(true));
        assert_not_on_executor();
    }
}
