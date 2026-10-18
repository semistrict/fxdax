// Copyright 2026 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The physical memory manager.
//!
//! Ported from `zircon/kernel/vm/pmm_node.rs` and `zircon/kernel/vm/pmm.rs`.
//!
//! The PMM owns every page of physical memory (see `physmap`) and hands them
//! out one at a time. Like Zircon's, it can ask callers that are able to wait
//! to do so when free memory is low (`ALLOC_FLAG_CAN_WAIT`), and signals an
//! event when free memory leaves a configured range, which is how the memory
//! watchdog learns about pressure.
//!
//! Deviations:
//! - One range of physical memory, whose memory is host mappings.
//! - fxdax: that memory is in arenas, each its own shared memory object (see `physmap`, TASK-43),
//!   which take it a chunk at a time. Each arena has its own free list, of the free pages of its
//!   chunks; a chunk whose pages are all free goes back to the node. Free memory, the pressure
//!   levels and eviction count every free page, whichever arena's: an arena that has no free page
//!   of its own when no chunk is free waits for eviction to free a chunk.
//! - No loaned pages, free-page fill checking, phys hand-off lists or
//!   compression.
//! - `set_free_memory_signal` takes an `Arc<Event>` rather than a raw
//!   pointer the caller must keep alive.

use crate::boot_options::BootOptions;
use crate::kernel::deadline::Deadline;
use crate::kernel::event::{AutounsignalEvent, Event};
use crate::kernel::types::PAddr;
use crate::vm::evictor::Evictor;
use crate::vm::page::{VmPage, VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use crate::vm::page_state::VmPageState;
use crate::vm::physmap::{ArenaId, CHUNK_SIZE, PHYS_BASE, PhysMemory};
use parking_lot::Mutex;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once, OnceLock};
use zx_status::Status;

/// No restrictions on which arena to allocate from.
pub const ALLOC_FLAG_ANY: u32 = 0;
/// The caller is able to wait and retry this allocation and so pmm allocation functions are allowed
/// to return ZX_ERR_SHOULD_WAIT, as opposed to ZX_ERR_NO_MEMORY, to indicate that the caller should
/// wait and try again. This is intended for the PMM to tell callers who are able to wait that
/// memory is low. The caller should not infer anything about memory state if it is told to wait, as
/// the PMM may tell it to wait for any reason.
pub const ALLOC_FLAG_CAN_WAIT: u32 = 1 << 0;

/// Whether a freed page should have its reuse delayed. A hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PmmOptDelayReuse {
    Default,
    Yes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AllocFailureType {
    #[default]
    None,
    Pmm,
    Heap,
    Other,
}

/// Information about an allocation failure, kept to aid diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllocFailure {
    pub r#type: AllocFailureType,
    pub size: usize,
    pub free_count: u64,
}

/// Controls the behavior of requests that have the PMM_ALLOC_FLAG_CAN_WAIT.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShouldWaitState {
    /// The PMM_ALLOC_FLAG_CAN_WAIT should never be followed and we will always attempt to perform
    /// the allocation, or fail with ZX_ERR_NO_MEMORY. This state is permanent and cannot be left.
    Never,
    /// Allocations do not need to be delayed, but the should_wait_free_pages_level should be
    /// monitored and once tripped should be delayed.
    OnceLevelTripped,
    /// State indicates that the level got tripped, and we should delay any allocations until the
    /// level is reset.
    UntilReset,
}

/// The pages of physical memory and their `VmPage` structures.
struct PmmPages {
    memory: PhysMemory,
    page_array: Box<[VmPage]>,
}

impl PmmPages {
    fn new(size: usize) -> Self {
        let memory = PhysMemory::new(size);
        let page_array = (0..size / page::SIZE)
            .map(|i| VmPage::new(PAddr(PHYS_BASE.0 + i * page::SIZE)))
            .collect();
        Self { memory, page_array }
    }

    fn page(&self, index: usize) -> VmPagePtr {
        VmPagePtr::new(NonNull::from(&self.page_array[index]))
    }

    fn index_of(&self, page: VmPagePtr) -> Option<usize> {
        let base = self.page_array.as_ptr() as usize;
        let offset = (page.as_raw() as usize).checked_sub(base)?;
        let index = offset / size_of::<VmPage>();
        (index < self.page_array.len()).then_some(index)
    }
}

struct PmmNodeLocked {
    /// fxdax: each arena's free pages, by arena id: the free pages of the chunks it has.
    free_lists: Vec<VmPageDoublyLinkedList>,
    /// fxdax: the chunks no arena has, whose pages are all free.
    free_chunks: Vec<usize>,
    /// fxdax: how many pages of each chunk are allocated.
    chunk_used: Vec<u32>,
    should_wait: ShouldWaitState,
    /// Below this number of free pages the PMM will transition into delaying allocations.
    should_wait_free_pages_level: u64,
    /// A record of the first time an allocation failure is reported to aid in diagnostics.
    first_alloc_failure: AllocFailure,
    /// If mem_signal is set, then once the available free memory falls outside of the
    /// defined lower and upper bound the signal is raised. This is a one-shot signal and is
    /// cleared after firing.
    mem_signal: Option<Arc<Event>>,
    mem_signal_lower_bound: u64,
    mem_signal_upper_bound: u64,
    /// The rng state for random waiting on allocations.
    random_should_wait_seed: u64,
}

/// A node of physical memory.
pub struct PmmNode {
    lock: Mutex<PmmNodeLocked>,
    // This is both an atomic and guarded by lock as we would like modifications to require the
    // lock, as logic in the system relies on the free_count not changing whilst the lock is
    // held, but also be an atomic so it can be correctly read without the lock.
    free_count: AtomicU64,
    /// The event acts a gate keeper for waking up threads waiting for allocations one at time.
    /// The event gets signalled when there MAY be pages available.
    may_allocate_evt: AutounsignalEvent,
    /// Indicates whether a PMM alloc call has ever failed with ZX_ERR_NO_MEMORY. Used to trigger
    /// an OOM response.  See |MemoryWatchdog::WorkerThread|.
    alloc_failed_no_mem: AtomicBool,
    /// fxdax: signalled when a chunk goes back to the node, for an arena waiting for one.
    chunk_freed_evt: AutounsignalEvent,
    arena: PmmPages,
    page_queues: PageQueues,
    evictor: Arc<Evictor>,
}

impl PmmNode {
    /// Bits kept zero at the bottom of a page index, for users of indices to store metadata in.
    pub const INDEX_ZERO_BITS: u32 = 3;

    fn new(size: usize) -> Self {
        let arena = PmmPages::new(size);
        let chunks = arena.memory.chunk_count();
        let free_count = AtomicU64::new(arena.page_array.len() as u64);
        Self {
            lock: Mutex::new(PmmNodeLocked {
                free_lists: vec![VmPageDoublyLinkedList::new()],
                // Lower chunks first.
                free_chunks: (0..chunks).rev().collect(),
                chunk_used: vec![0; chunks],
                should_wait: ShouldWaitState::OnceLevelTripped,
                should_wait_free_pages_level: 0,
                first_alloc_failure: AllocFailure::default(),
                mem_signal: None,
                mem_signal_lower_bound: 0,
                mem_signal_upper_bound: 0,
                random_should_wait_seed: 0x5eed,
            }),
            free_count,
            may_allocate_evt: AutounsignalEvent::new(true),
            alloc_failed_no_mem: AtomicBool::new(false),
            chunk_freed_evt: AutounsignalEvent::new(false),
            arena,
            page_queues: PageQueues::new(),
            evictor: Arc::new(Evictor::new()),
        }
    }

    pub fn evictor(&self) -> &Arc<Evictor> {
        &self.evictor
    }

    pub fn page_queues(&self) -> &PageQueues {
        &self.page_queues
    }

    pub fn phys_memory(&self) -> &PhysMemory {
        &self.arena.memory
    }

    /// Converts the number returned by page_to_index() back to a VmPagePtr pointer.
    ///
    /// # Safety
    ///
    /// The `index` must be a valid PMM page index.
    pub unsafe fn index_to_page(&self, index: u32) -> VmPagePtr {
        self.arena.page(((index >> Self::INDEX_ZERO_BITS) - 1) as usize)
    }

    /// Returns compressed representation a page_t*, with the following characteristics:
    /// - zeros in the last INDEX_ZERO_BITS bits, used by clients to store metadata.
    /// - The value 0 is never returned, it can be used as "no page" marker.
    pub fn page_to_index(&self, page: VmPagePtr) -> u32 {
        match self.arena.index_of(page) {
            Some(index) => ((index + 1) as u32) << Self::INDEX_ZERO_BITS,
            None => 0,
        }
    }

    /// Converts the number returned by page_to_index() back to a PAddr.
    ///
    /// # Safety
    ///
    /// The `index` must be a valid PMM page index.
    pub unsafe fn index_to_paddr(&self, index: u32) -> PAddr {
        let page_ix = ((index >> Self::INDEX_ZERO_BITS) - 1) as usize;
        PAddr(PHYS_BASE.0 + page_ix * page::SIZE)
    }

    /// Converts physical address to VmPagePtr if it belongs to the arena.
    pub fn paddr_to_page(&self, addr: PAddr) -> Option<VmPagePtr> {
        let index = addr.0.checked_sub(PHYS_BASE.0)? / page::SIZE;
        (index < self.arena.page_array.len()).then(|| self.arena.page(index))
    }

    /// Allocates a single physical page from this node, of the default arena.
    pub fn alloc_page(&self, alloc_flags: u32) -> Result<VmPagePtr, Status> {
        self.alloc_page_in(ArenaId::DEFAULT, alloc_flags)
    }

    /// fxdax: allocates a single physical page from this node, of `arena`.
    pub fn alloc_page_in(&self, arena: ArenaId, alloc_flags: u32) -> Result<VmPagePtr, Status> {
        let mut locked = self.lock.lock();
        if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
            && self.should_delay_allocation_locked(&mut locked)
        {
            return Err(Status::SHOULD_WAIT);
        }

        let Some(page) = self.take_page_locked(&mut locked, arena) else {
            let free_count = self.free_count.load(Ordering::Relaxed);
            if free_count > 0 {
                // Free pages, but of other arenas' chunks: one goes back once eviction frees
                // all of its pages.
                drop(locked);
                return Err(self.starved(arena, alloc_flags));
            }
            // Allocation failures from the regular free list are likely to become user-visible.
            self.report_alloc_failure_locked(
                &mut locked,
                AllocFailure { r#type: AllocFailureType::Pmm, size: 1, free_count: 0 },
            );
            return Err(Status::NO_MEMORY);
        };
        // SAFETY: The page was just taken from the free list under the lock.
        unsafe { Self::alloc_page_helper_locked(page) };
        self.decrement_free_count_locked(&mut locked, 1);
        Ok(page)
    }

    /// fxdax: a free page of `arena`'s, taking a chunk for it if it has none.
    fn take_page_locked(&self, locked: &mut PmmNodeLocked, arena: ArenaId) -> Option<VmPagePtr> {
        let index = arena.0 as usize;
        if locked.free_lists.len() <= index {
            locked.free_lists.resize_with(index + 1, VmPageDoublyLinkedList::new);
        }
        if locked.free_lists[index].is_empty() {
            let chunk = locked.free_chunks.pop()?;
            self.arena.memory.assign_chunk(chunk, arena);
            let pages = CHUNK_SIZE / page::SIZE;
            for page in chunk * pages..(chunk + 1) * pages {
                locked.free_lists[index].push_back(self.arena.page(page));
            }
        }
        let page = locked.free_lists[index].pop_front()?;
        locked.chunk_used[self.arena.memory.chunk_of(page.paddr())] += 1;
        Some(page)
    }

    /// fxdax: puts `page`, free, back on its arena's free list, at the front unless `delay_reuse`,
    /// and its chunk back to the node if the page was the last of it allocated.
    fn put_page_locked(&self, locked: &mut PmmNodeLocked, page: VmPagePtr, delay_reuse: bool) {
        let chunk = self.arena.memory.chunk_of(page.paddr());
        let arena = self.arena.memory.chunk_arena(chunk).expect("a page of no arena's");
        let list = &mut locked.free_lists[arena.0 as usize];
        if delay_reuse {
            list.push_back(page);
        } else {
            list.push_front(page);
        }
        locked.chunk_used[chunk] -= 1;
        if locked.chunk_used[chunk] == 0 {
            let pages = CHUNK_SIZE / page::SIZE;
            for index in chunk * pages..(chunk + 1) * pages {
                list.erase(self.arena.page(index));
            }
            self.arena.memory.release_chunk(chunk);
            locked.free_chunks.push(chunk);
            self.chunk_freed_evt.signal();
        }
    }

    /// fxdax: what an allocation of `arena`'s that found no page of its own, and no chunk, when
    /// other arenas have free pages, fails with: eviction is asked to free a chunk's worth, which
    /// one that can wait waits for (`wait_for_single_page_allocation_in`).
    fn starved(&self, arena: ArenaId, alloc_flags: u32) -> Status {
        let _ = arena;
        self.evictor.evict_asynchronous(
            CHUNK_SIZE as u64,
            0,
            crate::vm::evictor::EvictionLevel::IncludeNewest,
            crate::vm::evictor::Output::NoPrint,
        );
        if alloc_flags & ALLOC_FLAG_CAN_WAIT != 0 { Status::SHOULD_WAIT } else { Status::NO_MEMORY }
    }

    /// Allocates `count` physical pages of the default arena, adding them to the tail of `list`.
    pub fn alloc_pages(
        &self,
        count: usize,
        alloc_flags: u32,
        list: &mut VmPageDoublyLinkedList,
    ) -> Result<(), Status> {
        self.alloc_pages_in(ArenaId::DEFAULT, count, alloc_flags, list)
    }

    /// fxdax: allocates `count` physical pages of `arena`, adding them to the tail of `list`.
    pub fn alloc_pages_in(
        &self,
        arena: ArenaId,
        count: usize,
        alloc_flags: u32,
        list: &mut VmPageDoublyLinkedList,
    ) -> Result<(), Status> {
        if count == 0 {
            return Ok(());
        } else if count == 1 {
            let page = self.alloc_page_in(arena, alloc_flags)?;
            list.push_back(page);
            return Ok(());
        }

        let mut locked = self.lock.lock();
        let free_count = self.free_count.load(Ordering::Relaxed);
        if count as u64 > free_count {
            if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                && locked.should_wait != ShouldWaitState::Never
            {
                return Err(Status::SHOULD_WAIT);
            }
            // Allocation failures from the regular free list are likely to become user-visible.
            self.report_alloc_failure_locked(
                &mut locked,
                AllocFailure { r#type: AllocFailureType::Pmm, size: count, free_count },
            );
            return Err(Status::NO_MEMORY);
        }

        self.decrement_free_count_locked(&mut locked, count as u64);

        if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
            && self.should_delay_allocation_locked(&mut locked)
        {
            self.increment_free_count_locked(&mut locked, count as u64);
            return Err(Status::SHOULD_WAIT);
        }

        let mut alloc_list = VmPageDoublyLinkedList::new();
        for _ in 0..count {
            let Some(page) = self.take_page_locked(&mut locked, arena) else {
                // Free pages, but too many of other arenas' chunks: those taken go back.
                while let Some(page) = alloc_list.pop_front() {
                    self.put_page_locked(&mut locked, page, false);
                }
                self.increment_free_count_locked(&mut locked, count as u64);
                drop(locked);
                return Err(self.starved(arena, alloc_flags));
            };
            alloc_list.push_back(page);
        }
        for page in alloc_list.iter() {
            // SAFETY: The page was just taken from the free list under the lock.
            unsafe { Self::alloc_page_helper_locked(page) };
        }
        drop(locked);
        list.splice(&mut alloc_list);
        Ok(())
    }

    /// Frees a single physical page back to this node.
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid and they are the owner.
    pub unsafe fn free_page(&self, page: VmPagePtr, delay_reuse: PmmOptDelayReuse) {
        let mut locked = self.lock.lock();
        // pages freed individually shouldn't be in a queue.
        // SAFETY: The caller owns the page.
        debug_assert!(unsafe { !page.in_container() });
        // SAFETY: The caller owns the page.
        unsafe { Self::free_page_helper_locked(page) };
        self.increment_free_count_locked(&mut locked, 1);
        self.put_page_locked(&mut locked, page, delay_reuse == PmmOptDelayReuse::Yes);
    }

    /// Frees every page on `list` back to this node.
    ///
    /// # Safety
    ///
    /// Caller guarantees that all pages in list are valid and they are the owner.
    pub unsafe fn free_list(
        &self,
        list: &mut VmPageDoublyLinkedList,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let mut locked = self.lock.lock();
        let mut count = 0;
        for page in list.iter() {
            // SAFETY: The caller owns the pages.
            unsafe { Self::free_page_helper_locked(page) };
            count += 1;
        }
        // In order, as a list spliced to the front of a free list would be.
        let mut pages = Vec::new();
        while let Some(page) = list.pop_front() {
            pages.push(page);
        }
        let delay = delay_reuse == PmmOptDelayReuse::Yes;
        if delay {
            for page in pages {
                self.put_page_locked(&mut locked, page, true);
            }
        } else {
            for page in pages.into_iter().rev() {
                self.put_page_locked(&mut locked, page, false);
            }
        }
        self.increment_free_count_locked(&mut locked, count);
    }

    /// fxdax: the number of chunks no arena has.
    pub fn count_free_chunks(&self) -> usize {
        self.lock.lock().free_chunks.len()
    }

    /// Return count of unallocated physical pages in this node.
    pub fn count_free_pages(&self) -> u64 {
        self.free_count.load(Ordering::Relaxed)
    }

    /// Return amount of physical memory in this node, in bytes.
    pub fn count_total_bytes(&self) -> u64 {
        (self.arena.page_array.len() * page::SIZE) as u64
    }

    /// Configures the free memory bounds and allows for setting a one shot signal as well as a
    /// level where allocations should start being delayed.
    ///
    /// The event is signaled once the number of PMM free pages falls outside of the range given by
    /// |free_lower_bound| and |free_upper_bound|. As the event is one shot, one signaled this must
    /// be called again to configure a new range. If the number of free pages is already outside the
    /// requested bound then this method fails (returns false) and no event is setup. In this case
    /// the caller should recalculate a correct bounds and try again.
    ///
    /// In addition to exiting the provided memory bounds, the event will also get signaled on the
    /// first time an allocation fails (i.e. the first time at which has_alloc_failed_no_mem would
    /// return true).
    ///
    /// |delay_allocations_level| is the number of PMM free pages below which the PMM will
    /// transition to delaying allocations that can wait, i.e. those with PMM_ALLOC_FLAG_CAN_WAIT.
    /// This transition is sticky, and even if pages are freed to go back above this line,
    /// allocations will remain delayed until this method is called again to re-set the level. For
    /// this reason, and since there is only a single common Event, the |delay_allocations_level|
    /// must either be <= the |free_lower_bound|, ensuring that the caller will have been notified
    /// and can respond by freeing memory and/or setting a new level, or |delay_allocations_level|
    /// can be UINT64_MAX, indicating allocations should start and remain delayed.
    pub fn set_free_memory_signal(
        &self,
        free_lower_bound: u64,
        free_upper_bound: u64,
        delay_allocations_pages: u64,
        event: Arc<Event>,
    ) -> bool {
        let mut locked = self.lock.lock();
        // Ensure delay allocations is valid.
        debug_assert!(
            delay_allocations_pages <= free_lower_bound || delay_allocations_pages == u64::MAX
        );
        let free_count = self.count_free_pages();
        if free_count < free_lower_bound || free_count > free_upper_bound {
            return false;
        }
        if delay_allocations_pages == u64::MAX {
            self.trip_free_pages_level_locked(&mut locked);
        } else if locked.should_wait == ShouldWaitState::UntilReset {
            self.may_allocate_evt.signal();
            locked.should_wait = ShouldWaitState::OnceLevelTripped;
        }
        locked.should_wait_free_pages_level = delay_allocations_pages;
        locked.mem_signal_lower_bound = free_lower_bound;
        locked.mem_signal_upper_bound = free_upper_bound;
        locked.mem_signal = Some(event);
        true
    }

    /// Waits the system to exit low memory state and then attempts to allocate, from the default
    /// arena.
    pub fn wait_for_single_page_allocation(&self, deadline: Deadline) -> Result<VmPagePtr, Status> {
        self.wait_for_single_page_allocation_in(ArenaId::DEFAULT, deadline)
    }

    /// Waits the system to exit low memory state and then attempts to allocate.
    ///
    /// To prevent herding problem, and because allocation compete for the `PmmNode::lock_` anyway,
    /// only one thread is woken up a time, only if the previous thread successfully allocated.
    ///
    /// In normal conditions, when the system is in low memory state, this method will return
    /// `ZX_ERR_TIMED_OUT` if the system didn't transition fast enough. If we run into a TOC to TOU,
    /// for the system race `ZX_ERR_SHOULD_WAIT` will be returned, that is the system transitioned
    /// out and back into low memory state before we managed to perform the allocation.
    ///
    /// fxdax: from `arena`, which, if it is starved, having no page of its own when no chunk is
    /// free, waits for a chunk to go back to the node, and then fails with `SHOULD_WAIT`, for the
    /// caller to try again.
    pub fn wait_for_single_page_allocation_in(
        &self,
        arena: ArenaId,
        deadline: Deadline,
    ) -> Result<VmPagePtr, Status> {
        // If we have been instructed to never wait, skip waiting on the event entirely to
        // prevent blocking. The Never state is final.
        if self.may_wait() {
            // Let the caller handle the error and retry if necessary.
            self.may_allocate_evt.wait(&deadline)?;
        }
        let result = self.allocate_after_wait(arena);
        if result == Err(Status::SHOULD_WAIT) && self.is_starved(arena) {
            self.chunk_freed_evt.wait(&deadline)?;
        }
        result
    }

    /// Waits for the system to exit low memory state and then attempts to allocate, as
    /// `wait_for_single_page_allocation` does, as a future rather than by blocking the calling
    /// thread.
    pub async fn wait_for_single_page_allocation_async(
        &self,
        arena: ArenaId,
        deadline: Deadline,
    ) -> Result<VmPagePtr, Status> {
        if self.may_wait() {
            self.may_allocate_evt.wait_async(deadline).await?;
        }
        let result = self.allocate_after_wait(arena);
        if result == Err(Status::SHOULD_WAIT) && self.is_starved(arena) {
            self.chunk_freed_evt.wait_async(deadline).await?;
        }
        result
    }

    /// fxdax: whether `arena` has no free page of its own while no chunk is free.
    fn is_starved(&self, arena: ArenaId) -> bool {
        let locked = self.lock.lock();
        locked.free_chunks.is_empty()
            && locked.free_lists.get(arena.0 as usize).is_none_or(|list| list.is_empty())
    }

    /// Whether an allocation may wait, unless the node has been told never to.
    fn may_wait(&self) -> bool {
        self.lock.lock().should_wait != ShouldWaitState::Never
    }

    /// Attempts an allocation after waiting for the system to exit low memory state.
    fn allocate_after_wait(&self, arena: ArenaId) -> Result<VmPagePtr, Status> {
        // Try to allocate the page now, it may fail sporadically, since there is no guarantee that
        // by the time we attempt to allocate the pages are still available.
        let res = self.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT);

        // Normally we would only signal in the `ZX_OK` case, i.e. when we are in an allocation-able
        // state. Otherwise we would wake up another thread just for it to receive
        // `ZX_ERR_SHOULD_WAIT` and immediately go back to waiting on the event. However, we also
        // signal when `should_wait == Never`, to ensure all waiting threads are woken up, and in the
        // random wait mode, where we may block in a non low memory state.
        let should_signal = match self.lock.lock().should_wait {
            ShouldWaitState::Never => true,
            ShouldWaitState::UntilReset => false,
            ShouldWaitState::OnceLevelTripped => res.is_ok() || res == Err(Status::SHOULD_WAIT),
        };
        if should_signal {
            self.may_allocate_evt.signal();
        }
        res
    }

    /// Tells the node to stop returning SHOULD_WAIT.
    pub fn stop_returning_should_wait(&self) {
        self.lock.lock().should_wait = ShouldWaitState::Never;
        self.may_allocate_evt.signal();
    }

    /// Returns whether an allocation has failed with NO_MEMORY.
    pub fn has_alloc_failed_no_mem(&self) -> bool {
        self.alloc_failed_no_mem.load(Ordering::Relaxed)
    }

    /// Retrieves information from the first allocation failure.
    pub fn get_first_alloc_failure(&self) -> AllocFailure {
        self.lock.lock().first_alloc_failure
    }

    fn should_delay_allocation_locked(&self, locked: &mut PmmNodeLocked) -> bool {
        match locked.should_wait {
            ShouldWaitState::UntilReset => return true,
            ShouldWaitState::Never => return false,
            ShouldWaitState::OnceLevelTripped => {}
        }
        if BootOptions::get().pmm_alloc_random_should_wait {
            // A xorshift step stands in for rand_r; one allocation in ten is delayed.
            let seed = &mut locked.random_should_wait_seed;
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            if seed.is_multiple_of(10) {
                return true;
            }
        }
        false
    }

    fn report_alloc_failure_locked(&self, locked: &mut PmmNodeLocked, failure: AllocFailure) {
        // Update before signaling the MemoryWatchdog to ensure it observes the update.
        //
        // |alloc_failed_no_mem| latches so only need to invoke the callback once.
        let first_time = !self.alloc_failed_no_mem.swap(true, Ordering::Relaxed);
        if first_time {
            let mut first = failure;
            // Record the free_count_ only for non-Pmm types. For PMM alloc failures, we know
            // exactly what the free count was at the time, because we use that to determine whether
            // the allocation should fail.
            if failure.r#type != AllocFailureType::Pmm {
                first.free_count = self.free_count.load(Ordering::Relaxed);
            }
            locked.first_alloc_failure = first;
            if locked.mem_signal.is_some() {
                Self::signal_free_memory_change_locked(locked);
            }
        }
    }

    fn signal_free_memory_change_locked(locked: &mut PmmNodeLocked) {
        let event = locked.mem_signal.take().expect("mem_signal is set");
        event.signal();
    }

    fn trip_free_pages_level_locked(&self, locked: &mut PmmNodeLocked) {
        if locked.should_wait == ShouldWaitState::OnceLevelTripped {
            locked.should_wait = ShouldWaitState::UntilReset;
            self.may_allocate_evt.unsignal();
        }
    }

    fn increment_free_count_locked(&self, locked: &mut PmmNodeLocked, count: u64) {
        let new_free_count = self.free_count.fetch_add(count, Ordering::Relaxed) + count;
        if locked.mem_signal.is_some() && new_free_count > locked.mem_signal_upper_bound {
            Self::signal_free_memory_change_locked(locked);
        }
    }

    fn decrement_free_count_locked(&self, locked: &mut PmmNodeLocked, count: u64) {
        debug_assert!(self.free_count.load(Ordering::Relaxed) >= count);
        let new_free_count = self.free_count.fetch_sub(count, Ordering::Relaxed) - count;
        if locked.should_wait == ShouldWaitState::OnceLevelTripped
            && new_free_count < locked.should_wait_free_pages_level
        {
            self.trip_free_pages_level_locked(locked);
        }
        if locked.mem_signal.is_some() && new_free_count < locked.mem_signal_lower_bound {
            Self::signal_free_memory_change_locked(locked);
        }
    }

    /// # Safety
    ///
    /// `page` must have just been removed from the free list with the lock held.
    unsafe fn alloc_page_helper_locked(page: VmPagePtr) {
        debug_assert!(page.is_free());
        // Here we transition the page from FREE->ALLOC, completing the transfer of ownership
        // from the PmmNode to the caller. This must be done under lock, and more specifically
        // the same lock acquisition that removes the page from the free list, as both being the
        // free list, or being in the ALLOC state, indicate ownership by the PmmNode.
        // SAFETY: Guaranteed by the caller.
        unsafe { page.set_state(VmPageState::Alloc) };
    }

    /// # Safety
    ///
    /// The caller must own `page` and hold the lock.
    unsafe fn free_page_helper_locked(page: VmPagePtr) {
        debug_assert!(!page.is_free());
        // SAFETY: Guaranteed by the caller.
        unsafe {
            debug_assert!(
                page.state() != VmPageState::Object
                    || (page.object().pin_count() == 0 && page.object().get_object().is_null())
            );
            // mark it free. This makes the page owned the PmmNode, even though it may not be in any
            // page list, since the page is findable via the arena, and so we must ensure to:
            // 1. Be performing set_state here under the lock
            // 2. Place the page in the free list and cease referring to the page before ever
            //    dropping lock
            page.set_state(VmPageState::Free);
        }
    }
}

/// Returns the PMM node, booting it and the kernel's background services on first use.
pub fn node() -> &'static PmmNode {
    static NODE: OnceLock<PmmNode> = OnceLock::new();
    static INIT_HOOKS: Once = Once::new();
    let node = NODE.get_or_init(|| {
        // SAFETY: sysconf has no preconditions.
        let host_page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        assert_eq!(
            host_page_size as usize,
            page::SIZE,
            "the kernel's page size must match the host's"
        );
        let size = page::round_down(BootOptions::get().memory_bytes as usize);
        PmmNode::new(size)
    });
    INIT_HOOKS.call_once(|| crate::init::init_hooks(node));
    node
}

thread_local! {
    /// fxdax: the arena the VMOs this thread creates take their pages from ([`with_arena`]).
    static CURRENT_ARENA: std::cell::Cell<ArenaId> = const { std::cell::Cell::new(ArenaId::DEFAULT) };
}

/// fxdax: runs `f` with the VMOs it creates on this thread taking their pages from `arena`, for
/// as long as they live.
pub fn with_arena<R>(arena: ArenaId, f: impl FnOnce() -> R) -> R {
    let before = CURRENT_ARENA.replace(arena);
    let result = f();
    CURRENT_ARENA.set(before);
    result
}

/// fxdax: the arena the VMOs this thread creates take their pages from.
pub fn current_arena() -> ArenaId {
    CURRENT_ARENA.get()
}

/// fxdax: a new arena of physical memory (see `physmap`), which holds no page.
pub fn create_arena() -> ArenaId {
    node().phys_memory().create_arena()
}

/// fxdax: removes `arena`, none of whose pages may be allocated. Not the default.
pub fn remove_arena(arena: ArenaId) {
    let node = node();
    let locked = node.lock.lock();
    // An arena whose pages are all free has given its chunks back.
    debug_assert!(locked.free_lists.get(arena.0 as usize).is_none_or(|list| list.is_empty()));
    node.phys_memory().remove_arena(arena);
}

/// Allocates a single page, returning it and its physical address.
pub fn alloc_page(flags: u32) -> Result<(VmPagePtr, PAddr), Status> {
    let page = node().alloc_page(flags)?;
    Ok((page, page.paddr()))
}

/// Allocates `count` pages onto the end of `list`.
pub fn alloc_pages(
    count: usize,
    flags: u32,
    list: &mut VmPageDoublyLinkedList,
) -> Result<(), Status> {
    node().alloc_pages(count, flags, list)
}

/// Frees a page.
///
/// # Safety
///
/// The caller must own `page`.
pub unsafe fn free_page(page: VmPagePtr) {
    // SAFETY: Guaranteed by the caller.
    unsafe { node().free_page(page, PmmOptDelayReuse::Default) }
}

/// Frees every page on `list`.
///
/// # Safety
///
/// The caller must own the pages.
pub unsafe fn free_list(list: &mut VmPageDoublyLinkedList) {
    // SAFETY: Guaranteed by the caller.
    unsafe { node().free_list(list, PmmOptDelayReuse::Default) }
}

/// Returns the page at physical address `paddr`.
pub fn paddr_to_vm_page(paddr: PAddr) -> Option<VmPagePtr> {
    node().paddr_to_page(paddr)
}

/// Returns the number of free pages.
pub fn count_free_pages() -> u64 {
    node().count_free_pages()
}

/// Returns the number of bytes of physical memory.
pub fn count_total_bytes() -> u64 {
    node().count_total_bytes()
}
