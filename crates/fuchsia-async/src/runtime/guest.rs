// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// fxdax's own: the runtime in the guest kernel (target_os = "none", TASK-26.6), in place of the
// portable runtime's tokio: tasks run on the kernel's workers, timers are the kernel's, and
// executors poll their main future on the calling thread, which sleeps between polls
// (guest/rust/linux-rt). Scopes are the portable runtime's.

#[path = "portable/scope.rs"]
pub mod scope;

pub mod task {
    use futures::future::{AbortHandle as FuturesAbortHandle, Abortable, Aborted};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// What aborts a task whose handle someone else holds, as a scope does.
    pub type AbortHandle = FuturesAbortHandle;

    /// A future on the kernel's workers, which an abort handle can stop.
    type Inner<T> = linux_rt::Task<Result<T, Aborted>>;

    fn spawn_abortable<T: Send + 'static>(
        future: impl Future<Output = T> + Send + 'static,
    ) -> (Inner<T>, AbortHandle) {
        let (abort, registration) = FuturesAbortHandle::new_pair();
        (linux_rt::spawn(Abortable::new(future, registration)), abort)
    }

    /// A handle to a task, which cancels it when dropped, unless detached.
    pub struct Task<T> {
        task: Option<Inner<T>>,
        abort: AbortHandle,
    }

    impl<T> std::fmt::Debug for Task<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Task")
        }
    }

    impl<T> Task<T> {
        /// Returns a `JoinHandle`, which detaches the task when dropped.
        pub fn detach_on_drop(mut self) -> JoinHandle<T> {
            JoinHandle { task: self.task.take(), abort: self.abort.clone() }
        }
    }

    impl<T: 'static> Task<T> {
        /// Spawns `fut` on the kernel's workers.
        pub fn spawn(fut: impl Future<Output = T> + Send + 'static) -> Self
        where
            T: Send,
        {
            let (task, abort) = spawn_abortable(fut);
            Self { task: Some(task), abort }
        }

        /// Panics: the guest kernel has no single-threaded executors, whose tasks need not be
        /// `Send`.
        pub fn local(_fut: impl Future<Output = T> + 'static) -> Self {
            panic!("the guest kernel has no local executors")
        }

        /// Lets the task run to completion without waiting for it.
        pub fn detach(mut self) {
            self.task.take().unwrap().detach();
        }

        /// Aborts the task, and returns a future of its output, if it completed first.
        pub fn abort(mut self) -> impl Future<Output = Option<T>> {
            let task = self.task.take().unwrap();
            async move { task.cancel().await.and_then(Result::ok) }
        }

        pub(crate) fn abort_handle(&self) -> AbortHandle {
            self.abort.clone()
        }
    }

    impl<T> Future for Task<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
            match Pin::new(self.task.as_mut().unwrap()).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(output)) => Poll::Ready(output),
                // All codepaths for aborting a task consume it.
                Poll::Ready(Err(Aborted)) => unreachable!("Task was polled after being aborted"),
            }
        }
    }

    impl<T> Drop for Task<T> {
        fn drop(&mut self) {
            // Dropping the kernel's task cancels it.
            drop(self.task.take());
        }
    }

    /// The error of a task that was aborted before it completed.
    #[derive(Debug)]
    pub struct JoinError(());

    impl JoinError {
        /// Whether the task was cancelled: always, as tasks don't unwind.
        pub fn is_cancelled(&self) -> bool {
            true
        }

        /// Whether the task panicked: never, as panics abort.
        pub fn is_panic(&self) -> bool {
            false
        }
    }

    impl std::fmt::Display for JoinError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("task was cancelled")
        }
    }

    impl std::error::Error for JoinError {}

    /// A handle to a task, which detaches it when dropped, as tokio's does.
    pub struct JoinHandle<T> {
        task: Option<Inner<T>>,
        abort: AbortHandle,
    }

    impl<T> std::fmt::Debug for JoinHandle<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("JoinHandle")
        }
    }

    /// The same as `JoinHandle`, as on the portable runtime.
    pub type CancelableJoinHandle<T> = JoinHandle<T>;

    impl<T> JoinHandle<T> {
        /// Aborts the task.
        pub fn abort(&self) {
            self.abort.abort();
        }

        /// Whether the task is done.
        pub fn is_finished(&self) -> bool {
            self.task.as_ref().is_none_or(|task| task.is_finished())
        }
    }

    impl<T> Future for JoinHandle<T> {
        type Output = Result<T, JoinError>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(self.task.as_mut().unwrap())
                .poll(cx)
                .map(|output| output.map_err(|Aborted| JoinError(())))
        }
    }

    impl<T> Drop for JoinHandle<T> {
        fn drop(&mut self) {
            if let Some(task) = self.task.take() {
                task.detach();
            }
        }
    }

    /// Yields to the executor once.
    pub async fn yield_now() {
        let mut yielded = false;
        std::future::poll_fn(|cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }

    /// Runs `f`, which blocks, on one of the kernel's workers.
    pub fn unblock<T: 'static + Send>(
        f: impl 'static + Send + FnOnce() -> T,
    ) -> impl 'static + Send + Future<Output = T> {
        Task::spawn(async move { f() })
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

    /// An executor, which polls its main future on the calling thread, while the kernel's
    /// workers run the tasks it spawns.
    pub struct SendExecutor(());

    impl SendExecutor {
        /// Polls `main_future` to completion on the calling thread.
        pub fn run<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            linux_rt::block_on(main_future)
        }
    }

    /// Builds `SendExecutor`s. The kernel's workers are its threads, however many are asked for.
    #[derive(Default)]
    pub struct SendExecutorBuilder(());

    impl SendExecutorBuilder {
        /// A builder.
        pub fn new() -> Self {
            Self(())
        }

        /// Ignored: the kernel's workers run tasks.
        pub fn num_threads(self, _num_threads: u8) -> Self {
            self
        }

        /// Ignored.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// The executor.
        pub fn build(self) -> SendExecutor {
            SendExecutor(())
        }
    }

    /// An executor of futures that need not be `Send`, on the calling thread. Its tasks still run
    /// on the kernel's workers.
    pub struct LocalExecutor(());

    impl LocalExecutor {
        /// An executor.
        pub fn new() -> Self {
            Self(())
        }

        /// Polls `main_future` to completion on the calling thread.
        pub fn run_singlethreaded<F>(&mut self, main_future: F) -> F::Output
        where
            F: Future,
        {
            linux_rt::block_on(main_future)
        }
    }

    impl Default for LocalExecutor {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Builds `LocalExecutor`s.
    #[derive(Default)]
    pub struct LocalExecutorBuilder {}

    impl LocalExecutorBuilder {
        /// A builder.
        pub fn new() -> Self {
            Self {}
        }

        /// Ignored.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// The executor.
        pub fn build(self) -> LocalExecutor {
            LocalExecutor::new()
        }
    }

    /// A `LocalExecutor`, for tests.
    pub struct TestExecutor {
        executor: LocalExecutor,
    }

    impl TestExecutor {
        /// An executor.
        pub fn new() -> Self {
            Self { executor: LocalExecutor::new() }
        }
    }

    impl Default for TestExecutor {
        fn default() -> Self {
            Self::new()
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

    /// Builds `TestExecutor`s.
    #[derive(Default)]
    pub struct TestExecutorBuilder {}

    impl TestExecutorBuilder {
        /// A builder.
        pub fn new() -> Self {
            Self {}
        }

        /// Ignored.
        pub fn allow_interrupts(self, _allow_interrupts: bool) -> Self {
            self
        }

        /// The executor.
        pub fn build(self) -> TestExecutor {
            TestExecutor::new()
        }
    }

    /// A boxed future, as on the portable runtime.
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

    /// An asynchronous timer: the kernel's, started when first polled.
    #[must_use = "futures do nothing unless polled"]
    pub struct Timer {
        inner: Option<linux_rt::Timer>,
        deadline: std::time::Instant,
    }

    impl std::fmt::Debug for Timer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Timer").field("deadline", &self.deadline).finish()
        }
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
            Timer { inner: None, deadline: instant }
        }
    }

    impl Future for Timer {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let deadline = self.deadline;
            let timer = self.inner.get_or_insert_with(|| {
                linux_rt::Timer::new(deadline.saturating_duration_since(std::time::Instant::now()))
            });
            Pin::new(timer).poll(cx)
        }
    }
}
