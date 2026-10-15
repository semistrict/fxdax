//! Futures in the kernel: `block_on` polls one on the calling thread, which sleeps between polls,
//! and `spawn` polls one on the kernel's workers, a poll per wake.

use crate::ffi;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::task::Wake;
use async_task::Runnable;
use core::ffi::c_void;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::{Context, Poll, Waker};

/// Polls `future` to completion on the calling thread, which sleeps while the future waits.
///
/// The future is polled from the heap, as it may be larger than the kernel's stacks can spare.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let parker = Arc::new(Parker::new());
    let waker = Waker::from(parker.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        parker.park();
    }
}

/// Lets a thread wait until it is woken, spinning briefly before it sleeps (the kernel's side,
/// `fxfs_rt_park`): a wake lets a `park` return, and wakes meanwhile are taken with it.
struct Parker(NonNull<ffi::Parker>);

// SAFETY: A parker may be parked on by one thread and unparked from any context.
unsafe impl Send for Parker {}
// SAFETY: As above.
unsafe impl Sync for Parker {}

impl Parker {
    fn new() -> Self {
        // SAFETY: Allocates a parker, which `drop` frees.
        let parker = unsafe { ffi::fxfs_rt_parker_new() };
        Self(NonNull::new(parker).expect("no memory for a parker"))
    }

    fn park(&self) {
        // SAFETY: The parker is live.
        unsafe { ffi::fxfs_rt_park(self.0.as_ptr()) }
    }
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // SAFETY: The parker is live; unparking does not sleep.
        unsafe { ffi::fxfs_rt_unpark(self.0.as_ptr()) }
    }
}

impl Drop for Parker {
    fn drop(&mut self) {
        // SAFETY: Nothing else holds the parker.
        unsafe { ffi::fxfs_rt_parker_free(self.0.as_ptr()) }
    }
}

/// A future running on the kernel's workers. Dropping it cancels the future; `detach` lets it run
/// on.
#[must_use = "dropping a task cancels it"]
pub struct Task<T>(async_task::Task<T>);

impl<T> Task<T> {
    /// Lets the future run to completion without waiting for it.
    pub fn detach(self) {
        self.0.detach();
    }

    /// Whether the future is done.
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    /// Cancels the future, and returns its output if it completed first.
    pub async fn cancel(self) -> Option<T> {
        self.0.cancel().await
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Runs `future` on the kernel's workers.
pub fn spawn<F>(future: F) -> Task<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (runnable, task) = async_task::spawn(future, schedule);
    runnable.schedule();
    Task(task)
}

/// Runs `future`, which tasks wait for, on the kernel's workers at a high priority, as a
/// filesystem's kernel threads run.
pub fn spawn_urgent<F>(future: F) -> Task<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (runnable, task) = async_task::spawn(future, schedule_urgent);
    runnable.schedule();
    Task(task)
}

/// Queues a task's next poll on a worker. Wakers call this from any context, so it does not
/// allocate with Rust's allocator, which may sleep.
fn schedule(runnable: Runnable) {
    let runnable = runnable.into_raw();
    // SAFETY: `run` takes back the runnable, once.
    let ret = unsafe { ffi::fxfs_rt_schedule(run, runnable.as_ptr().cast()) };
    // Losing the poll would leave the task waiting forever.
    assert_eq!(ret, 0, "no memory to schedule a task");
}

/// As `schedule`, at a high priority.
fn schedule_urgent(runnable: Runnable) {
    let runnable = runnable.into_raw();
    // SAFETY: `run` takes back the runnable, once.
    let ret = unsafe { ffi::fxfs_rt_schedule_urgent(run, runnable.as_ptr().cast()) };
    assert_eq!(ret, 0, "no memory to schedule a task");
}

extern "C" fn run(runnable: *mut c_void) {
    // SAFETY: `schedule` gave up the runnable, which is not null, for this call.
    let runnable = unsafe { Runnable::<()>::from_raw(NonNull::new_unchecked(runnable.cast())) };
    runnable.run();
}
