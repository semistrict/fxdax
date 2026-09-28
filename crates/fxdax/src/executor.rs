//! An executor of its own for Fxfs's futures, run for callers that block, such as a virtio-fs
//! device's worker thread.

use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use std::future::Future;

/// Fxfs's executor: a fuchsia-async `SendExecutor` on threads of its own.
pub(crate) struct Executor {
    /// Futures to run, until the executor is dropped.
    jobs: mpsc::UnboundedSender<BoxFuture<'static, ()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Executor {
    /// Starts an executor with `threads` threads.
    pub fn new(threads: u8) -> Self {
        let (jobs, mut receiver) = mpsc::unbounded::<BoxFuture<'static, ()>>();
        let thread = std::thread::Builder::new()
            .name("fxdax executor".into())
            .spawn(move || {
                fuchsia_async::SendExecutorBuilder::new().num_threads(threads).build().run(
                    async move {
                        while let Some(job) = receiver.next().await {
                            fuchsia_async::Task::spawn(job).detach();
                        }
                    },
                )
            })
            .expect("failed to start the executor's thread");
        Self { jobs, thread: Some(thread) }
    }

    /// Runs `future` on the executor, blocking the calling thread until it completes.
    ///
    /// # Panics
    ///
    /// If called on one of the executor's threads, which would block the futures it waits for.
    pub fn run<T: Send + 'static>(&self, future: impl Future<Output = T> + Send + 'static) -> T {
        assert_not_on_executor();
        let (sender, receiver) = oneshot::channel();
        self.jobs
            .unbounded_send(
                async move {
                    let _ = sender.send(future.await);
                }
                .boxed(),
            )
            .expect("the executor has stopped");
        futures::executor::block_on(receiver).expect("the executor dropped a future")
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        self.jobs.close_channel();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the executor's thread panicked");
        }
    }
}

/// Asserts that the calling thread isn't an executor's, whose futures a blocking call there could
/// wait for, such as a page request from the kernel when stream I/O faults.
pub(crate) fn assert_not_on_executor() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "blocking call on an executor thread");
}
