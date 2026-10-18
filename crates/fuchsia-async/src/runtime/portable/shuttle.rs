// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: the portable runtime's tasks, executors and timers on Shuttle (awslabs/shuttle,
// Apache-2.0), with the `shuttle` feature, in place of tokio's, so that Shuttle's scheduler
// chooses every interleaving of a test's tasks, such as Fxfs's in the follower simulation
// (TASK-42.1). Their API is the portable runtime's (`portable.rs`).
//
// Under Shuttle time doesn't pass: a timer never fires, as Shuttle's own timed waits never time
// out. What only time drives, such as Fxfs's periodic tasks, doesn't happen; a wait that only a
// timer would end is a deadlock Shuttle reports.

pub mod task {
    use core::task::{Context, Poll};
    use std::future::Future;
    use std::mem::ManuallyDrop;
    use std::pin::Pin;

    pub(crate) use shuttle::future::AbortHandle;
    pub use shuttle::future::{JoinHandle, JoinHandle as CancelableJoinHandle};

    /// Yields to the other tasks: a scheduling point.
    pub async fn yield_now() {
        shuttle::future::yield_now().await
    }

    impl<T> From<JoinHandle<T>> for Task<T> {
        fn from(task: JoinHandle<T>) -> Self {
            Self { task, abort_on_drop: true }
        }
    }

    /// A handle to a task, as the portable runtime's: dropped, it cancels the task.
    #[derive(Debug)]
    pub struct Task<T> {
        task: JoinHandle<T>,
        abort_on_drop: bool,
    }

    impl<T> Task<T> {
        /// Returns a `JoinHandle` which will have detach-on-drop semantics.
        pub fn detach_on_drop(self) -> JoinHandle<T> {
            let this = ManuallyDrop::new(self);
            // SAFETY: We are bypassing our drop implementation.
            unsafe { std::ptr::read(&this.task) }
        }
    }

    impl<T: 'static> Task<T> {
        /// Spawns a new `Send` task.
        pub fn spawn(fut: impl Future<Output = T> + Send + 'static) -> Self
        where
            T: Send,
        {
            Self { task: shuttle::future::spawn(fut), abort_on_drop: true }
        }

        /// Spawns a new non-`Send` task.
        pub fn local(fut: impl Future<Output = T> + 'static) -> Self {
            Self { task: shuttle::future::spawn_local(fut), abort_on_drop: true }
        }

        /// Detaches the task, which runs to completion.
        pub fn detach(mut self) {
            self.abort_on_drop = false;
        }

        /// Aborts the task, returning a future that resolves once it is aborted, with its output
        /// if it completed first.
        pub fn abort(mut self) -> impl Future<Output = Option<T>> {
            self.task.abort();
            async move { (&mut self.task).await.ok() }
        }

        pub(crate) fn abort_handle(&self) -> AbortHandle {
            self.task.abort_handle()
        }
    }

    impl<T> Future for Task<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match Pin::new(&mut self.task).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(v)) => Poll::Ready(v),
                // All codepaths for canceling/aborting a task consume said task.
                Poll::Ready(Err(_)) => unreachable!("Task was polled after being cancelled"),
            }
        }
    }

    impl<T> Drop for Task<T> {
        fn drop(&mut self) {
            if self.abort_on_drop {
                self.task.abort();
            }
        }
    }

    /// Runs a blocking function as a task of its own.
    pub fn unblock<T: 'static + Send>(
        f: impl 'static + Send + FnOnce() -> T,
    ) -> impl 'static + Send + Future<Output = T> {
        let task = shuttle::future::spawn(async move { f() });
        async move { task.await.expect("an unblocked function is never cancelled") }
    }
}

pub mod executor {
    use crate::Timer;
    use crate::runtime::WakeupTime;
    use futures::future::BoxFuture;
    use std::future::Future;
    use std::ops::{Deref, DerefMut};
    use std::pin::Pin;
    use std::task::{Context, Poll};

    pub use std::time::Duration as MonotonicDuration;
    /// A time relative to the executor's clock.
    pub use std::time::Instant as MonotonicInstant;

    impl WakeupTime for MonotonicInstant {
        fn into_timer(self) -> Timer {
            Timer::from(self)
        }
    }

    /// An executor: Shuttle's, which runs a future on the current Shuttle thread, its tasks
    /// interleaved as Shuttle's scheduler chooses.
    pub struct SendExecutor {}

    impl SendExecutor {
        /// Runs a single future to completion.
        pub fn run<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            shuttle::future::block_on(main_future)
        }
    }

    /// A builder for `SendExecutor`.
    #[derive(Default)]
    pub struct SendExecutorBuilder {}

    impl SendExecutorBuilder {
        /// Creates a new builder used for constructing a `SendExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets the number of threads, which Shuttle's executor has one of.
        pub fn num_threads(self, _num_threads: u8) -> Self {
            self
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// Builds the `SendExecutor`, consuming this `SendExecutorBuilder`.
        pub fn build(self) -> SendExecutor {
            SendExecutor {}
        }
    }

    /// A single-threaded executor: Shuttle's.
    #[derive(Default)]
    pub struct LocalExecutor {}

    impl LocalExecutor {
        /// Create a new executor.
        pub fn new() -> Self {
            Self {}
        }

        /// Runs a single future to completion.
        pub fn run_singlethreaded<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future,
        {
            shuttle::future::block_on(main_future)
        }
    }

    /// A builder for `LocalExecutor`.
    #[derive(Default)]
    pub struct LocalExecutorBuilder {}

    impl LocalExecutorBuilder {
        /// Creates a new builder used for constructing a `LocalExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// Builds the `LocalExecutor`, consuming this `LocalExecutorBuilder`.
        pub fn build(self) -> LocalExecutor {
            LocalExecutor::default()
        }
    }

    /// A single-threaded executor for testing: Shuttle's.
    #[derive(Default)]
    pub struct TestExecutor {
        executor: LocalExecutor,
    }

    impl TestExecutor {
        /// Create a new executor for testing.
        pub fn new() -> Self {
            Self::default()
        }
    }

    /// A builder for `TestExecutor`.
    #[derive(Default)]
    pub struct TestExecutorBuilder {}

    impl TestExecutorBuilder {
        /// Creates a new builder used for constructing a `TestExecutor`.
        pub fn new() -> Self {
            Self::default()
        }

        /// Sets whether the executor should support binding interrupts.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// Builds the `TestExecutor`, consuming this `TestExecutorBuilder`.
        pub fn build(self) -> TestExecutor {
            TestExecutor::new()
        }
    }

    impl Deref for TestExecutor {
        type Target = LocalExecutor;

        fn deref(&self) -> &Self::Target {
            &self.executor
        }
    }

    impl DerefMut for TestExecutor {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.executor
        }
    }

    /// As the portable runtime's: a boxed future.
    pub struct SpawnableFuture<'a, O>(BoxFuture<'a, O>);

    impl<'a, O> SpawnableFuture<'a, O> {
        /// Returns a new SpawnableFuture.
        pub fn new(future: impl Future<Output = O> + Send + 'a) -> Self {
            Self(Box::pin(future))
        }
    }

    impl<O> Future for SpawnableFuture<'_, O> {
        type Output = O;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut self.0).poll(cx)
        }
    }
}

pub mod timer {
    use crate::WakeupTime;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// An asynchronous timer, which, under Shuttle, where time doesn't pass, never fires.
    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    pub struct Timer {
        _deadline: std::time::Instant,
    }

    impl Timer {
        /// Create a new timer scheduled to fire at `time`.
        pub fn new(time: impl WakeupTime) -> Self {
            time.into_timer()
        }
    }

    impl From<std::time::Duration> for Timer {
        fn from(duration: std::time::Duration) -> Self {
            Self::from(crate::MonotonicInstant::now() + duration)
        }
    }

    impl From<crate::MonotonicInstant> for Timer {
        fn from(instant: crate::MonotonicInstant) -> Self {
            Timer { _deadline: instant }
        }
    }

    impl Future for Timer {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }
}
