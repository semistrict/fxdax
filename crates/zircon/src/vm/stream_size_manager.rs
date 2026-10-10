//! Coordinating changes to a VMO's stream size.
//!
//! Ported from `zircon/kernel/vm/stream_size_manager.rs`.
//!
//! Deviations:
//! - The queues hold each operation's id, type and size, rather than being intrusive lists of
//!   pinned operations whose fields the manager's lock guards. Operations need not be pinned, and
//!   the lock is the manager's mutex guard rather than lock tokens.
//! - An operation blocked until it is at the head of a queue waits on a condition variable the
//!   manager shares, which dequeuing notifies, rather than on an event of its own. A future may
//!   wait in place of a thread (`begin_write`), with its waker in the queues (see
//!   `kernel::wakers`), which the lock's guard wakes once it has released the lock.
//! - An operation keeps its own type and size, which only its owner changes, besides the copies in
//!   the queues other operations read, so `update_stream_size_from_progress` checks it without a
//!   lock token it makes up. They are atomics, so that a future may own an operation on any
//!   thread.
//! - An operation can be abandoned (`abandon`), as a future that owns one is when it is dropped
//!   before finishing: it commits the progress it published, or is cancelled.

use crate::kernel::wakers::Wakers;
use parking_lot::{Condvar, Mutex, MutexGuard};
use std::collections::VecDeque;
use std::future::poll_fn;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::task::Poll;
use zx_status::Status;

/// The type of operation being managed by `StreamSizeManager`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Write = 0,
    Read = 1,
    SetSize = 2,
    Append = 3,
}

/// An operation's entry in a queue.
#[derive(Debug, Clone, Copy)]
struct Entry {
    id: u64,
    op_type: OperationType,
    /// Holds the target size. For appends, this will only be valid once the operation is at the
    /// head of the queue.
    size: u64,
}

/// The queues of a `StreamSizeManager`, which its lock guards.
#[derive(Default)]
pub struct Queues {
    // These queues are usually very shallow, unless stream clients call many operations
    // concurrently.
    write_q: VecDeque<Entry>,
    read_q: VecDeque<Entry>,
    /// The futures waiting for their operations to be at the head of the queues.
    wakers: Wakers,
    /// Whether an operation was dequeued, so that the waiting futures are to be woken once the
    /// lock is released.
    dequeued: bool,
}

impl Queues {
    /// Updates the size of the operation `id` in the queues holding it.
    fn set_size(&mut self, id: u64, size: u64) {
        for entry in self.write_q.iter_mut().chain(self.read_q.iter_mut()) {
            if entry.id == id {
                entry.size = size;
            }
        }
    }

    fn contains(queue: &VecDeque<Entry>, id: u64) -> bool {
        queue.iter().any(|entry| entry.id == id)
    }

    /// The entries in front of the operation `id` in `queue`.
    fn ahead_of(queue: &VecDeque<Entry>, id: u64) -> impl Iterator<Item = &Entry> {
        queue.iter().take_while(move |entry| entry.id != id)
    }

    fn is_front(queue: &VecDeque<Entry>, id: u64) -> bool {
        queue.front().is_some_and(|entry| entry.id == id)
    }

    fn remove(queue: &mut VecDeque<Entry>, id: u64) {
        let index = queue.iter().position(|entry| entry.id == id).expect("operation is queued");
        queue.remove(index);
    }

    /// Whether the operation `id` is at the head of each queue it is in.
    fn is_head(&self, id: u64) -> bool {
        [&self.write_q, &self.read_q]
            .into_iter()
            .all(|queue| !Self::contains(queue, id) || Self::is_front(queue, id))
    }
}

/// The lock of a `StreamSizeManager`, held. Releasing it wakes the futures waiting on the queues
/// if an operation was dequeued while it was held, as their wakers must not be woken under it.
pub struct StreamSizeGuard<'a>(ManuallyDrop<MutexGuard<'a, Queues>>);

impl Deref for StreamSizeGuard<'_> {
    type Target = Queues;

    fn deref(&self) -> &Queues {
        &self.0
    }
}

impl DerefMut for StreamSizeGuard<'_> {
    fn deref_mut(&mut self) -> &mut Queues {
        &mut self.0
    }
}

impl StreamSizeGuard<'_> {
    /// Runs `f` with the lock released, as `MutexGuard::unlocked` does, having woken the futures
    /// if an operation was dequeued.
    pub fn unlocked<T>(guard: &mut Self, f: impl FnOnce() -> T) -> T {
        let wake = std::mem::take(&mut guard.dequeued).then(|| guard.wakers.take());
        MutexGuard::unlocked(&mut guard.0, || {
            drop(wake);
            f()
        })
    }
}

impl Drop for StreamSizeGuard<'_> {
    fn drop(&mut self) {
        let wake = std::mem::take(&mut self.dequeued).then(|| self.wakers.take());
        // SAFETY: The guard is dropped only here, and not used again.
        unsafe { ManuallyDrop::drop(&mut self.0) };
        drop(wake);
    }
}

/// `Operation` is a structure to ensure operations related to stream size are committed in order.
/// `Operation` is intended to be used as a stack-allocated structure.
///
/// Currently, an operation maps 1:1 with the thread it is executing on and thus, can be
/// considered owned by that thread, or by the future it is part of.
///
/// Notes:
///  * The type must either be committed or cancelled before destruction. Otherwise, the destructor
///    will panic.
pub struct Operation<'a> {
    parent: &'a StreamSizeManager,
    id: u64,
    // Only the owner queues, changes and dequeues the operation, so these are its to read without
    // the lock. The queues hold copies of the type and size for other operations.
    valid: AtomicBool,
    op_type: AtomicU8,
    size: AtomicU64,

    /// Tracks any stream size updated this operation performed. This is used to ensure that an
    /// operation is not cancelled or shrunk beyond any published stream size updates, and to
    /// commit them when it is abandoned.
    committed_stream_size: AtomicU64,
}

impl<'a> Operation<'a> {
    pub fn new(parent: &'a StreamSizeManager) -> Self {
        Self {
            parent,
            id: parent.next_id.fetch_add(1, Ordering::Relaxed),
            valid: AtomicBool::new(false),
            op_type: AtomicU8::new(OperationType::Read as u8),
            size: AtomicU64::new(0),
            committed_stream_size: AtomicU64::new(0),
        }
    }

    // Only the owner reads and writes these, so relaxed accesses suffice.
    fn is_valid(&self) -> bool {
        self.valid.load(Ordering::Relaxed)
    }

    fn set_valid(&self, valid: bool) {
        self.valid.store(valid, Ordering::Relaxed);
    }

    fn op_type(&self) -> OperationType {
        match self.op_type.load(Ordering::Relaxed) {
            0 => OperationType::Write,
            1 => OperationType::Read,
            2 => OperationType::SetSize,
            3 => OperationType::Append,
            other => unreachable!("operation type {other}"),
        }
    }

    fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    fn committed_stream_size(&self) -> u64 {
        self.committed_stream_size.load(Ordering::Relaxed)
    }

    /// Indicates whether the operation is valid.
    pub fn is_valid_locked(&self, _guard: &StreamSizeGuard<'_>) -> bool {
        self.is_valid()
    }

    fn initialize(&self, guard: &mut StreamSizeGuard<'_>, size: u64, op_type: OperationType) {
        debug_assert!(!self.is_valid());
        self.set_valid(true);
        self.op_type.store(op_type as u8, Ordering::Relaxed);
        self.size.store(size, Ordering::Relaxed);
        let entry = Entry { id: self.id, op_type, size };
        match op_type {
            OperationType::Write | OperationType::Append => guard.write_q.push_back(entry),
            OperationType::Read => guard.read_q.push_back(entry),
            OperationType::SetSize => {
                guard.write_q.push_back(entry);
                guard.read_q.push_back(entry);
            }
        }
    }

    /// Gets the operation's type while holding the parent lock.
    pub fn get_type_locked(&self, _guard: &StreamSizeGuard<'_>) -> OperationType {
        self.op_type()
    }

    /// Sets the operation's size, and its copy in the queues.
    fn set_size_locked(&self, guard: &mut StreamSizeGuard<'_>, size: u64) {
        self.size.store(size, Ordering::Relaxed);
        guard.set_size(self.id, size);
    }

    /// Gets the stream size that the operation will expand to once it is completed.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn get_size_locked(&self, _guard: &StreamSizeGuard<'_>) -> u64 {
        debug_assert!(self.is_valid());
        // Reading the size for `Append` operations should only occur after it's been initialized.
        debug_assert!(self.op_type() != OperationType::Append || self.size() > 0);
        self.size()
    }

    /// Shrinks the size of the operation.
    ///
    /// Only size shrinks are allowed, since the concurrency of other operations are gated on the
    /// largest potential size of operations in front of it.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    ///  * The `new_size` passed in must be greater than 0.
    ///  * The `new_size` passed in must be less than or equal to the current size.
    ///  * This must only be called for `OperationType::Append` and `OperationType::Write` ops.
    pub fn shrink_size_locked(&self, guard: &mut StreamSizeGuard<'_>, new_size: u64) {
        debug_assert!(self.is_valid());
        // If `new_size` is 0, the operation should be cancelled instead.
        debug_assert!(new_size > 0);
        // This function may only be called on expanding stream write operations.
        let op_type = self.op_type();
        assert!(op_type == OperationType::Append || op_type == OperationType::Write);
        assert!(new_size <= self.size());
        debug_assert!(new_size >= self.committed_stream_size());

        self.set_size_locked(guard, new_size);
    }

    /// Commits the operation's effects on the stream size.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn commit_locked(&self, guard: &mut StreamSizeGuard<'_>) {
        debug_assert!(self.is_valid());
        self.parent.commit_and_dequeue_operation_locked(self, guard);
    }

    /// Cancels the operation and does not commit any changes to the stream size.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn cancel_locked(&self, guard: &mut StreamSizeGuard<'_>) {
        debug_assert!(self.is_valid());
        debug_assert!(self.committed_stream_size() == 0);
        self.parent.dequeue_operation_locked(self, guard);
    }

    /// Finishes the operation if it is still queued, as a future that owns it must when it is
    /// dropped before finishing it: commits the stream size its progress published, if any, and
    /// otherwise cancels it.
    pub fn abandon(&self) {
        if !self.is_valid() {
            return;
        }
        let mut guard = self.parent.lock();
        match self.committed_stream_size() {
            0 => self.cancel_locked(&mut guard),
            committed => {
                self.shrink_size_locked(&mut guard, committed);
                self.commit_locked(&mut guard);
            }
        }
    }

    /// Updates the stream size when progress is made from the operation. Once this has been called
    /// it is invalid to call `cancel_locked` or to call `shrink_size_locked` with a size less than
    /// the stream size provided here.
    ///
    /// Note:
    ///  * This may only be called on a valid `Append` or `Write` operation.
    ///  * The stream size must be larger than the current stream size.
    pub fn update_stream_size_from_progress(&self, new_stream_size: u64) {
        debug_assert!(self.is_valid());
        let op_type = self.op_type();
        debug_assert!(op_type == OperationType::Write || op_type == OperationType::Append);
        debug_assert!(new_stream_size <= self.size());
        debug_assert!(new_stream_size > self.parent.get_stream_size());
        self.committed_stream_size.store(new_stream_size, Ordering::Relaxed);
        self.parent.set_stream_size(new_stream_size);
    }
}

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        debug_assert!(!self.is_valid(), "Operation destructed without cancelling or committing!");
    }
}

/// `StreamSizeManager` is a struct that helps coordinate multiple, potentially concurrent changes
/// to a VMO's stream size without needing to serialize the I/O of those operations. This is done by
/// maintaining queues of outstanding operations, allowing concurrent execution of the operations,
/// and then committing the stream size effects of those operations in a particular order. This idea
/// is similar to the re-order buffer in Tomasulo's algorithm.
///
/// There are 2 ordering queues: the read queue and the write queue. Both queues hold their
/// respective namesake operations as well as shrink operations.
///
/// Read operations are permitted to read up to the smallest outstanding stream size, which can be
/// found as the minimum of the current stream size and all shrink operations. Upon completion,
/// reads will always commit without blocking behind other operations.
///
/// Write operations may extend stream size. Upon completion, a write will block until it is the
/// head of the write queue if the smallest outstanding stream size is less than its target size.
///
/// Set size operations are treated differently, depending on whether the operation will expand or
/// shrink the stream size. When expanding, set size ops are treated as write operations of the same
/// target size (see above). When shrinking, set size ops are treated as shrink operations and will
/// block until it is the head if any read or write operations that operate beyond the target size
/// are queued in front of the set size.
pub struct StreamSizeManager {
    lock: Mutex<Queues>,

    /// Notified when an operation is dequeued, so that the next may be at the head of its queue.
    ready: Condvar,

    // `stream_size` is not guarded by a lock because the queues above maintain that only one
    // operation can ever be mutating `stream_size` at any given point.
    //
    // Accessing this value should be done via `get_stream_size` and `set_stream_size`.
    stream_size: AtomicU64,

    next_id: AtomicU64,
}

impl StreamSizeManager {
    /// Create a `StreamSizeManager` with its initial stream size set to `stream_size`.
    pub fn create(stream_size: u64) -> Arc<Self> {
        Arc::new(Self {
            lock: Mutex::new(Queues::default()),
            ready: Condvar::new(),
            stream_size: AtomicU64::new(stream_size),
            next_id: AtomicU64::new(0),
        })
    }

    /// Acquires the stream size manager's lock.
    pub fn lock(&self) -> StreamSizeGuard<'_> {
        StreamSizeGuard(ManuallyDrop::new(self.lock.lock()))
    }

    /// Returns the current stream size.
    pub fn get_stream_size(&self) -> u64 {
        // Loads from the operation the stream size must be ordered with the acquire ordering to
        // ensure that all memory operations from the VMO (i.e. reads) after the load are not
        // reordered before reading the stream size. Otherwise, reads from the VMO before acquiring
        // stream size may not see data that was written to the VMO just before stream size was
        // updated (via `SetStreamSize`).
        self.stream_size.load(Ordering::Acquire)
    }

    /// Updates the stream size to a new value.
    ///
    /// Note that this function should only be called by internal functions, as stream size should
    /// only be modified by one operation at a time. This is enforced by the queues.
    fn set_stream_size(&self, new_stream_size: u64) {
        // Stores to the stream size must be ordered with release ordering to ensure that all memory
        // operations (i.e. writes) to the VMO are visible *before* updating stream size. Readers
        // must see valid data in the VMO if the region being read is within stream size. See
        // `GetStreamSize` as well.
        self.stream_size.store(new_stream_size, Ordering::Release);
    }

    /// Marks and registers the beginning of a read operation.
    ///
    /// Returns the maximum size of the stream that should be read.
    pub fn begin_read_locked(
        &self,
        guard: &mut StreamSizeGuard<'_>,
        target_size: u64,
        op: &Operation<'_>,
    ) -> u64 {
        // Allow reads up to the smallest outstanding size.
        // Other concurrent, in-flight operations may or may not complete before this read, so it
        // is okay to be more conservative here and only read up to the guaranteed valid region.
        let mut stream_size_limit = self.get_stream_size();
        for item in guard.read_q.iter() {
            if item.op_type == OperationType::SetSize {
                stream_size_limit = core::cmp::min(item.size, stream_size_limit);
            }
        }
        stream_size_limit = core::cmp::min(target_size, stream_size_limit);

        op.initialize(guard, stream_size_limit, OperationType::Read);

        stream_size_limit
    }

    /// Marks and registers the beginning of a write operation.
    ///
    /// If the write results in an expansion of the stream size, returns the previous stream size
    /// from which the write expands. The gap from the previous stream size to where the write
    /// begins likely needs to be zeroed out.
    ///
    /// Notes:
    ///  * This function may block until other conflicting operations complete.
    ///  * This function may drop and reacquire the lock guarded by `guard`.
    pub fn begin_write_locked(
        &self,
        guard: &mut StreamSizeGuard<'_>,
        target_size: u64,
        op: &Operation<'_>,
    ) -> Option<u64> {
        if !self.queue_write_locked(guard, target_size, op) {
            return None;
        }
        self.block_until_head_locked(guard, op);
        self.expanded_from(target_size)
    }

    /// Marks and registers the beginning of a write operation, as `begin_write_locked` does, as a
    /// future that waits rather than blocking the calling thread, and returns the lock, held.
    pub async fn begin_write(
        &self,
        target_size: u64,
        op: &Operation<'_>,
    ) -> (StreamSizeGuard<'_>, Option<u64>) {
        let must_wait = {
            let mut guard = self.lock();
            if !self.queue_write_locked(&mut guard, target_size, op) {
                return (guard, None);
            }
            true
        };
        debug_assert!(must_wait);
        let guard = self.until_head(op).await;
        (guard, self.expanded_from(target_size))
    }

    /// Queues the write operation `op`, returning whether it must wait to be at the head of the
    /// queue before it may begin.
    fn queue_write_locked(
        &self,
        guard: &mut StreamSizeGuard<'_>,
        target_size: u64,
        op: &Operation<'_>,
    ) -> bool {
        op.initialize(guard, target_size, OperationType::Write);

        // Check if there are any set size operations in front of this that sets the stream size
        // smaller than `target_size`.
        let block_due_to_set = Queues::ahead_of(&guard.write_q, op.id)
            .any(|item| item.op_type == OperationType::SetSize && item.size < target_size);

        // If this write can potentially create a scenario where it expands stream, block until it
        // is the head of the queue.
        block_due_to_set || target_size > self.get_stream_size()
    }

    /// The stream size a write to `target_size`, at the head of the queue, expands from, if it
    /// does. The stream size must be re-read once at the head, since the lock was dropped while
    /// waiting, and stream size may have been modified by the operations in front of it.
    fn expanded_from(&self, target_size: u64) -> Option<u64> {
        let cur_stream_size = self.get_stream_size();
        (target_size > cur_stream_size).then_some(cur_stream_size)
    }

    /// Marks and registers the beginning of an append operation.
    ///
    /// Notes:
    ///  * This function may block until other conflicting operations complete.
    ///  * This function may drop and reacquire the lock guarded by `guard`.
    ///  * `append_size` must be greater than 0.
    pub fn begin_append_locked(
        &self,
        guard: &mut StreamSizeGuard<'_>,
        append_size: usize,
        op: &Operation<'_>,
    ) -> Result<(), Status> {
        debug_assert!(append_size > 0);

        // Temporarily initialize an `Append` operation with zero size until the size is known.
        op.initialize(guard, 0, OperationType::Append);

        // Block until head if there are any of the following operations preceding this one:
        //   * Appends or writes that exceed the current stream size.
        //   * Set size
        //
        // Effectively, this checks for any stream size modifying operations.
        //
        // It's okay to read the stream size once here, since the lock is held. This means that
        // stream size can only be increased if the front-most stream size modifying operation is
        // an expanding write or append. Not re-reading stream size and seeing a potentially smaller
        // stream size here is valid, since it will only pessimize (i.e. blocking until head) this
        // operation for a very small number of cases within an extremely narrow timing window.
        // There are no correctness issues with pessimization. Since the pessimizing case is so
        // rare, prefer reading once over continuously re-reading the atomic in a loop.
        let mut cur_stream_size = self.get_stream_size();

        let should_block = Queues::ahead_of(&guard.write_q, op.id).any(|item| {
            item.op_type == OperationType::SetSize
                || item.op_type == OperationType::Append
                || (item.op_type == OperationType::Write && item.size > cur_stream_size)
        });

        if should_block {
            self.block_until_head_locked(guard, op);

            // Must re-read the stream size here, since `block_until_head_locked` would have dropped
            // the lock, and stream size may have been modified by the operations in front of this
            // one.
            cur_stream_size = self.get_stream_size();
        }

        // Using the previously read stream size. In the case where this operation blocked until
        // it was the head, stream size was re-read with the lock held and with this operation at
        // the head of the queue (no other stream size mutating operations can proceed before this).
        // In all other cases, the previous loop verified that no stream size mutating operations
        // are in front of this operation.
        let Some(new_size) = cur_stream_size.checked_add(append_size as u64) else {
            // Dequeue operation since this change should not be committed.
            self.dequeue_operation_locked(op, guard);
            return Err(Status::OUT_OF_RANGE);
        };
        op.set_size_locked(guard, new_size);

        Ok(())
    }

    /// Marks and registers the beginning of an operation to set the stream size to a target size.
    ///
    /// Note that this function may drop and reacquire the lock guarded by `guard`.
    pub fn begin_set_stream_size_locked(
        &self,
        guard: &mut StreamSizeGuard<'_>,
        target_size: u64,
        op: &Operation<'_>,
    ) {
        op.initialize(guard, target_size, OperationType::SetSize);

        // Block until head if there are any of the following operations preceding this one:
        //   * Appends or writes that exceed either the current stream size or the target size.
        //      - If it exceeds the current stream size, the overlap is in the region in which the
        //        set size will zero content and the write will commit data.
        //      - If it exceeds the target size, the overlap is in the region in which the set size
        //        will invalidate pages/data and the write will commit data.
        //   * Reads that are reading at or beyond target size.
        //   * Set size
        //
        // It's okay to read the stream size once here, since the lock is held. This means that
        // stream size can only be increased if the front-most stream size modifying operation is
        // an expanding write or append. Not re-reading stream size and seeing a potentially smaller
        // stream size here is valid, since it will only pessimize (i.e. blocking until head) this
        // operation for a very small number of cases within an extremely narrow timing window.
        // There are no correctness issues with pessimization. Since the pessimizing case is so
        // rare, prefer reading once over continuously re-reading the atomic in a loop.
        let cur_stream_size = self.get_stream_size();

        let should_block = Queues::ahead_of(&guard.write_q, op.id).any(|item| {
            item.op_type == OperationType::SetSize
                || item.op_type == OperationType::Append
                || (item.op_type == OperationType::Write
                    && item.size > core::cmp::min(cur_stream_size, target_size))
        }) || Queues::ahead_of(&guard.read_q, op.id)
            .any(|item| item.op_type == OperationType::Read && item.size > target_size);

        if should_block {
            self.block_until_head_locked(guard, op);
        }
    }

    /// Blocks until the provided operation is at the head of the queue.
    ///
    /// Note that this function will drop the lock guarded by `guard` while blocking and
    /// reacquires the lock after.
    fn block_until_head_locked(&self, guard: &mut StreamSizeGuard<'_>, op: &Operation<'_>) {
        debug_assert!(std::ptr::eq(op.parent, self));
        // An operation at the front of a queue stays there until it is dequeued, so it is at the
        // head of both once it has been at the head of each.
        while op.is_valid() && !guard.is_head(op.id) {
            self.ready.wait(&mut guard.0);
        }
    }

    /// Waits until the provided operation is at the head of the queue, as
    /// `block_until_head_locked` does, as a future, and returns the lock, held.
    async fn until_head(&self, op: &Operation<'_>) -> StreamSizeGuard<'_> {
        debug_assert!(std::ptr::eq(op.parent, self));
        let mut key = None;
        let registered = Unregister { manager: self, key: &mut key };
        poll_fn(|cx| {
            let mut guard = self.lock();
            if !op.is_valid() || guard.is_head(op.id) {
                guard.wakers.remove(registered.key);
                return Poll::Ready(());
            }
            guard.wakers.register(registered.key, cx.waker());
            Poll::Pending
        })
        .await;
        drop(registered);
        self.lock()
    }

    fn commit_and_dequeue_operation_locked(
        &self,
        op: &Operation<'_>,
        guard: &mut StreamSizeGuard<'_>,
    ) {
        if !op.is_valid() {
            return;
        }

        match op.get_type_locked(guard) {
            OperationType::Write => {
                self.set_stream_size(core::cmp::max(
                    op.get_size_locked(guard),
                    self.get_stream_size(),
                ));
            }
            OperationType::Append | OperationType::SetSize => {
                self.set_stream_size(op.get_size_locked(guard));
            }
            OperationType::Read => {
                // No-op
            }
        }

        self.dequeue_operation_locked(op, guard);
    }

    /// Dequeues an `Operation`. This must only be called internally, once an `Operation` is
    /// committed or cancelled.
    fn dequeue_operation_locked(&self, op: &Operation<'_>, guard: &mut StreamSizeGuard<'_>) {
        debug_assert!(op.is_valid());
        debug_assert!(std::ptr::eq(op.parent, self));

        match op.get_type_locked(guard) {
            OperationType::Write | OperationType::Append => {
                Queues::remove(&mut guard.write_q, op.id);
            }
            OperationType::Read => {
                Queues::remove(&mut guard.read_q, op.id);
            }
            OperationType::SetSize => {
                Queues::remove(&mut guard.write_q, op.id);
                Queues::remove(&mut guard.read_q, op.id);
            }
        }
        op.set_valid(false);

        // Wake the operations waiting to be at the head of a queue, the next of which may now be:
        // threads now, futures once the lock is released.
        self.ready.notify_all();
        guard.dequeued = true;
    }
}

/// Unregisters a future's waker from a manager's queues when the future is dropped while waiting.
struct Unregister<'a, 'k> {
    manager: &'a StreamSizeManager,
    key: &'k mut Option<u64>,
}

impl Drop for Unregister<'_, '_> {
    fn drop(&mut self) {
        if self.key.is_some() {
            self.manager.lock().wakers.remove(self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Operation, StreamSizeManager};
    use std::task::Poll;

    /// Tests creating a StreamSizeManager and querying its initial size.
    #[test]
    fn create_and_get_size() {
        let ssm = StreamSizeManager::create(1024);
        assert_eq!(ssm.get_stream_size(), 1024);
    }

    /// Tests begin_read_locked and committing a read operation.
    #[test]
    fn read_operation() {
        let ssm = StreamSizeManager::create(2048);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        let limit = ssm.begin_read_locked(&mut guard, 4096, &op);
        assert_eq!(limit, 2048);
        assert!(op.is_valid_locked(&guard));
        assert_eq!(op.get_size_locked(&guard), 2048);

        op.commit_locked(&mut guard);
        assert!(!op.is_valid_locked(&guard));
        assert_eq!(ssm.get_stream_size(), 2048);
    }

    /// Tests begin_write_locked, shrinking the operation size, and committing.
    #[test]
    fn write_and_shrink_operation() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        let prev = ssm.begin_write_locked(&mut guard, 500, &op);
        assert_eq!(prev, Some(100));
        assert!(op.is_valid_locked(&guard));
        assert_eq!(op.get_size_locked(&guard), 500);

        op.shrink_size_locked(&mut guard, 300);
        assert_eq!(op.get_size_locked(&guard), 300);

        op.commit_locked(&mut guard);
        assert!(!op.is_valid_locked(&guard));
        assert_eq!(ssm.get_stream_size(), 300);
    }

    /// Tests cancelling a write operation.
    #[test]
    fn write_cancel_operation() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        let _prev = ssm.begin_write_locked(&mut guard, 500, &op);
        assert!(op.is_valid_locked(&guard));

        op.cancel_locked(&mut guard);
        assert!(!op.is_valid_locked(&guard));
        assert_eq!(ssm.get_stream_size(), 100);
    }

    /// Tests begin_append_locked and committing an append operation.
    #[test]
    fn append_operation() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        ssm.begin_append_locked(&mut guard, 50, &op).unwrap();
        assert!(op.is_valid_locked(&guard));
        assert_eq!(op.get_size_locked(&guard), 150);

        op.commit_locked(&mut guard);
        assert!(!op.is_valid_locked(&guard));
        assert_eq!(ssm.get_stream_size(), 150);
    }

    /// Tests begin_set_stream_size_locked and committing a set size operation.
    #[test]
    fn set_stream_size_operation() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        ssm.begin_set_stream_size_locked(&mut guard, 50, &op);
        assert!(op.is_valid_locked(&guard));
        assert_eq!(op.get_size_locked(&guard), 50);

        op.commit_locked(&mut guard);
        assert!(!op.is_valid_locked(&guard));
        assert_eq!(ssm.get_stream_size(), 50);
    }

    /// Tests updating stream size as progress is made during a write operation.
    #[test]
    fn update_progress() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);

        let mut guard = ssm.lock();
        let _prev = ssm.begin_write_locked(&mut guard, 500, &op);

        op.update_stream_size_from_progress(250);
        assert_eq!(ssm.get_stream_size(), 250);

        op.commit_locked(&mut guard);
        assert_eq!(ssm.get_stream_size(), 500);
    }

    /// Tests that an expanding write blocks until the shrinking set size in front of it commits,
    /// and then expands from the size that set.
    #[test]
    fn expanding_write_waits_for_set_size() {
        let ssm = StreamSizeManager::create(100);
        let set_size = Operation::new(&ssm);
        let mut guard = ssm.lock();
        ssm.begin_set_stream_size_locked(&mut guard, 50, &set_size);
        drop(guard);

        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                let op = Operation::new(&ssm);
                let mut guard = ssm.lock();
                let prev = ssm.begin_write_locked(&mut guard, 80, &op);
                op.commit_locked(&mut guard);
                prev
            });
            // The write can't commit until the set size ahead of it does.
            while !writer.is_finished() && ssm.lock().write_q.len() < 2 {
                std::thread::yield_now();
            }
            assert_eq!(ssm.get_stream_size(), 100);
            let mut guard = ssm.lock();
            set_size.commit_locked(&mut guard);
            drop(guard);
            assert_eq!(writer.join().unwrap(), Some(50));
        });
        assert_eq!(ssm.get_stream_size(), 80);
    }

    /// An expanding write waits, as a future, for the shrinking set size in front of it, without
    /// blocking the thread polling it, and then expands from the size that set.
    #[test]
    fn async_expanding_write_waits_for_set_size() {
        use futures::FutureExt;
        use futures::task::noop_waker_ref;
        use std::task::Context;

        let ssm = StreamSizeManager::create(100);
        let set_size = Operation::new(&ssm);
        ssm.begin_set_stream_size_locked(&mut ssm.lock(), 50, &set_size);

        let op = Operation::new(&ssm);
        let mut write = Box::pin(ssm.begin_write(80, &op));
        let mut cx = Context::from_waker(noop_waker_ref());
        assert!(write.poll_unpin(&mut cx).is_pending());
        set_size.commit_locked(&mut ssm.lock());
        let Poll::Ready((mut guard, prev)) = write.poll_unpin(&mut cx) else {
            panic!("the write is still waiting");
        };
        assert_eq!(prev, Some(50));
        op.commit_locked(&mut guard);
        drop(guard);
        drop(write);
        assert_eq!(ssm.get_stream_size(), 80);
    }

    /// A write that never has to wait begins at once.
    #[test]
    fn async_write_within_the_stream_does_not_wait() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);
        let (mut guard, prev) = futures::executor::block_on(ssm.begin_write(80, &op));
        assert_eq!(prev, None);
        op.commit_locked(&mut guard);
    }

    /// Dropping a waiting write leaves nothing registered, and abandoning it dequeues it.
    #[test]
    fn dropped_async_write_is_abandoned() {
        use futures::FutureExt;
        use futures::task::noop_waker_ref;
        use std::task::Context;

        let ssm = StreamSizeManager::create(100);
        let set_size = Operation::new(&ssm);
        ssm.begin_set_stream_size_locked(&mut ssm.lock(), 50, &set_size);
        let op = Operation::new(&ssm);
        {
            let mut write = Box::pin(ssm.begin_write(80, &op));
            assert!(write.poll_unpin(&mut Context::from_waker(noop_waker_ref())).is_pending());
        }
        assert!(ssm.lock().wakers.is_empty());
        op.abandon();
        assert_eq!(ssm.lock().write_q.len(), 1);
        set_size.commit_locked(&mut ssm.lock());
        assert_eq!(ssm.get_stream_size(), 50);
    }

    /// Abandoning an operation that published progress commits it.
    #[test]
    fn abandoning_commits_published_progress() {
        let ssm = StreamSizeManager::create(100);
        let op = Operation::new(&ssm);
        let _prev = ssm.begin_write_locked(&mut ssm.lock(), 500, &op);
        op.update_stream_size_from_progress(250);
        op.abandon();
        assert!(!op.is_valid_locked(&ssm.lock()));
        assert_eq!(ssm.get_stream_size(), 250);
        // Abandoning a finished operation does nothing.
        op.abandon();
    }
}
