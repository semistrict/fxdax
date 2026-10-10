//! An executor of its own for Fxfs's futures, which callers await from their own executors, or
//! threads.

use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Fxfs's executor: a fuchsia-async `SendExecutor` on threads of its own, which stops once it is
/// dropped and the futures it is running complete.
pub(crate) struct Executor {
    /// Futures to run, until the executor is dropped.
    jobs: mpsc::UnboundedSender<BoxFuture<'static, ()>>,
}

impl Executor {
    /// Starts an executor with `threads` threads.
    pub fn new(threads: u8) -> Self {
        let (jobs, mut receiver) = mpsc::unbounded::<BoxFuture<'static, ()>>();
        std::thread::Builder::new()
            .name("fxdax executor".into())
            .spawn(move || {
                fuchsia_async::SendExecutorBuilder::new().num_threads(threads).build().run(
                    async move {
                        // The jobs running, which the executor finishes before it stops: its
                        // runtime would drop them unfinished.
                        let mut running = FuturesUnordered::new();
                        loop {
                            futures::select! {
                                job = receiver.next() => match job {
                                    Some(job) => running.push(fuchsia_async::Task::spawn(job)),
                                    None => break,
                                },
                                () = running.select_next_some() => {}
                            }
                        }
                        while running.next().await.is_some() {}
                    },
                )
            })
            .expect("failed to start the executor's thread");
        Self { jobs }
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
        self.jobs
            .unbounded_send(
                OnExecutor(async move {
                    let _ = sender.send(future.await);
                })
                .boxed(),
            )
            .expect("the executor has stopped");
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
        self.jobs
            .unbounded_send(
                OnExecutor(async move {
                    let _ = sender.send(future.await);
                })
                .boxed(),
            )
            .expect("the executor has stopped");
        receiver.recv().expect("the executor dropped a future")
    }
}

impl Drop for Executor {
    /// Stops the executor's thread once the futures it is running complete, without waiting for
    /// them, as it may be dropped by one of them.
    fn drop(&mut self) {
        self.jobs.close_channel();
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
