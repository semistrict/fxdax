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
//!   which take it a chunk at a time: the free list is the arenas' (`arenas`), each arena's the
//!   free pages of its chunks. Free memory, the pressure levels and eviction count every free
//!   page, whichever arena's. An arena with no free page that can take no chunk, even with other
//!   arenas' spare chunks gone back for one, is starved (`Arenas::starved`): past its own cap, or,
//!   past the arenas' together (TASK-50.3) or out of chunks, with too few of their spares. Its
//!   allocations are relieved (`relief`), or, without relief, wait for eviction to free a chunk.
//!   An allocation of an arena starved at a cap as things stand (`Arenas::within_caps`), or
//!   starved and relieved, waits for room of its own rather than at the node's gate; but while the
//!   node delays allocations every allocation waits at the gate, as upstream (TASK-86). One of an
//!   arena removed fails with `NO_MEMORY`, whether or not it may wait:
//!   nothing makes the arena room, so a page's waits for nothing, even as the node delays
//!   allocations (TASK-79.36). So does one the host maps no chunk for, as the process makes as
//!   many mappings as it may (`physmap`, TASK-79.37), reported as the node's failure.
//! - fxdax: with memory never out (`OomBehavior::Evict`), an allocation that finds no free page
//!   at all is starved too, while eviction may free pages, which it frees on demand, whatever
//!   their age: upstream's fails at once, as its watchdog delays the allocations that can wait,
//!   and reclaims, before memory runs out (TASK-79.70).
//! - fxdax: a chunk given back goes back to the host as the node's lock is dropped, outside it
//!   (`NodeGuard`, TASK-92.35), as its system calls would hold every allocation up for as long
//!   under it; an allocation that it would make room for waits for it.
//! - fxdax: a page a hypervisor's second stage mapped since it was allocated, freed, is
//!   quarantined until every second stage has made the changes taking it away
//!   (`quarantine`, TASK-79.9), as the kernel waits for none.
//! - No loaned pages, free-page fill checking, phys hand-off lists or
//!   compression.
//! - `set_free_memory_signal` takes an `Arc<Event>` rather than a raw
//!   pointer the caller must keep alive.

use crate::boot_options::{BootOptions, OomBehavior};
use crate::kernel::deadline::Deadline;
use crate::kernel::event::{AutounsignalEvent, Event, Pulse};
use crate::kernel::types::PAddr;
use crate::sync::rules;
use crate::vm::evictor::Evictor;
use crate::vm::page::{VmPage, VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use crate::vm::page_state::VmPageState;
use crate::vm::physmap::{ArenaId, CHUNK_SIZE, PHYS_BASE, PhysMemory};
use arenas::{Arenas, Refused, Starved};
use parking_lot::{Mutex, MutexGuard};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Once, OnceLock};
use zx_status::Status;

mod arenas;
mod quarantine;
mod relief;

#[cfg(all(test, feature = "shuttle"))]
pub(crate) use quarantine::QuarantineForTest;
#[cfg(test)]
pub(crate) use quarantine::quarantine_for_test;
pub use quarantine::{Quarantined, quarantine_with};
pub use relief::{RELIEF_BYTES, relieve_with};

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
    /// fxdax: the free list, of the arenas' free pages, and the chunks no arena has.
    arenas: Arenas,
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

/// fxdax: the node's lock, held (TASK-92.35). As it is dropped, the host takes back the chunks the
/// arenas gave back under it, each without the lock, which it takes again to record what the host
/// did ([`Arenas::returned`]), and to tell allocations waiting for room: reserving a chunk's
/// addresses again and freeing its memory are system calls, which every allocation, of any arena's,
/// would otherwise wait for, and which none waits for now but one the chunk makes room for
/// (`Refused::GoingBack`).
struct NodeGuard<'a> {
    node: &'a PmmNode,
    locked: MutexGuard<'a, PmmNodeLocked>,
}

impl Deref for NodeGuard<'_> {
    type Target = PmmNodeLocked;

    fn deref(&self) -> &PmmNodeLocked {
        &self.locked
    }
}

impl DerefMut for NodeGuard<'_> {
    fn deref_mut(&mut self) -> &mut PmmNodeLocked {
        &mut self.locked
    }
}

impl Drop for NodeGuard<'_> {
    fn drop(&mut self) {
        let pages = &self.node.pages;
        for returning in self.locked.arenas.take_returning() {
            let returned = MutexGuard::unlocked(&mut self.locked, || returning.release(pages));
            self.locked.arenas.returned(pages, returned);
            self.node.room.pulse();
        }
    }
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
    /// fxdax: pulsed as an allocation of a starved arena may find room, or must fail now: a page or
    /// a chunk freed, a cap changed, relief's pass for an arena done, relief gone (TASK-50.3).
    /// Every waiter wakes, as the waiters of one arena's may not be those of the next's.
    room: Pulse,
    pages: PmmPages,
    page_queues: PageQueues,
    evictor: Arc<Evictor>,
}

impl PmmNode {
    /// Bits kept zero at the bottom of a page index, for users of indices to store metadata in.
    pub const INDEX_ZERO_BITS: u32 = 3;

    fn new(size: usize) -> Self {
        let pages = PmmPages::new(size);
        let free_count = AtomicU64::new(pages.page_array.len() as u64);
        let chunks = pages.memory.chunk_count();
        Self {
            lock: Mutex::new(PmmNodeLocked {
                arenas: Arenas::new(chunks),
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
            room: Pulse::new(),
            pages,
            page_queues: PageQueues::new(chunks),
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
        &self.pages.memory
    }

    /// fxdax: the chunks `arena` has, as the chunk table shows them without the node's lock, which
    /// may change as they are read: for the evictor to find the arena's own pages by
    /// (`Evictor::evict_arena_on_demand`, TASK-79.76), which checks each.
    pub(crate) fn arena_chunks(&self, arena: ArenaId) -> impl Iterator<Item = usize> + '_ {
        let memory = &self.pages.memory;
        (0..memory.chunk_count()).filter(move |&chunk| memory.chunk_arena(chunk) == Some(arena))
    }

    /// fxdax: the pages of chunk `chunk`, allocated or free.
    pub(crate) fn chunk_pages(&self, chunk: usize) -> impl Iterator<Item = VmPagePtr> + '_ {
        arenas::chunk_pages(chunk).map(|index| self.pages.page(index))
    }

    /// Acquires the node's lock (`NodeGuard`).
    fn locked(&self) -> NodeGuard<'_> {
        NodeGuard { node: self, locked: self.lock.lock() }
    }

    /// fxdax: drops `locked`, under which an allocation found chunks going back that would make it
    /// room (`Refused::GoingBack`), and waits for one to go back, or for other room, before it
    /// tries again (TASK-92.35): as it waited for the lock while the host took them back under it.
    fn wait_for_chunks_going_back(&self, locked: NodeGuard<'_>) {
        // Each goes back as its host calls return, which no deadline would hurry.
        let room = self.room.count();
        drop(locked);
        let waited = self.room.wait_past(room, &Deadline::infinite());
        debug_assert!(waited.is_ok());
    }

    /// Converts the number returned by page_to_index() back to a VmPagePtr pointer.
    ///
    /// # Safety
    ///
    /// The `index` must be a valid PMM page index.
    pub unsafe fn index_to_page(&self, index: u32) -> VmPagePtr {
        self.pages.page(((index >> Self::INDEX_ZERO_BITS) - 1) as usize)
    }

    /// Returns compressed representation a page_t*, with the following characteristics:
    /// - zeros in the last INDEX_ZERO_BITS bits, used by clients to store metadata.
    /// - The value 0 is never returned, it can be used as "no page" marker.
    pub fn page_to_index(&self, page: VmPagePtr) -> u32 {
        match self.pages.index_of(page) {
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
        (index < self.pages.page_array.len()).then(|| self.pages.page(index))
    }

    /// Allocates a single physical page from this node, of the default arena.
    pub fn alloc_page(&self, alloc_flags: u32) -> Result<VmPagePtr, Status> {
        self.alloc_page_in(ArenaId::DEFAULT, alloc_flags)
    }

    /// fxdax: allocates a single physical page from this node, of `arena`.
    pub fn alloc_page_in(&self, arena: ArenaId, alloc_flags: u32) -> Result<VmPagePtr, Status> {
        loop {
            let mut locked = self.locked();
            if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                && self.should_delay_allocation_locked(&mut locked)
            {
                // fxdax: but for an arena removed, whose allocations no wait makes room for.
                if locked.arenas.starved(arena) == Some(Starved::Removed) {
                    return Err(Status::NO_MEMORY);
                }
                return Err(Status::SHOULD_WAIT);
            }

            let page = match locked.arenas.take(&self.pages, arena) {
                Ok(page) => page,
                Err(Refused::Starved(starved)) => {
                    let free_count = self.free_count.load(Ordering::Relaxed);
                    // Free pages, but of other arenas' chunks: one goes back once eviction frees
                    // all of its pages. fxdax: none, and the arena is relieved, by evicting any
                    // arena's (TASK-50.3); or, memory never out, eviction frees pages of any
                    // arena's for it on demand (TASK-79.70).
                    if free_count > 0 || relief::started() || self.evicts_when_out() {
                        return Err(self.starved_status(locked, arena, starved, alloc_flags, 1));
                    }
                    // Allocation failures from the regular free list are likely to become
                    // user-visible.
                    self.report_alloc_failure_locked(
                        &mut locked,
                        AllocFailure { r#type: AllocFailureType::Pmm, size: 1, free_count: 0 },
                    );
                    return Err(Status::NO_MEMORY);
                }
                Err(Refused::Unmapped) => return Err(self.unmapped_status_locked(&mut locked, 1)),
                Err(Refused::GoingBack) => {
                    self.wait_for_chunks_going_back(locked);
                    continue;
                }
            };
            // SAFETY: The page was just taken from the free list under the lock.
            unsafe { Self::alloc_page_helper_locked(page) };
            self.decrement_free_count_locked(&mut locked, 1);
            return Ok(page);
        }
    }

    /// fxdax: caps the memory `arena` may take, in bytes, rounded down to chunks (2 MiB), or lifts
    /// its cap. Lowered under what it has, it takes no more until it has less (TASK-50.3).
    pub fn set_arena_cap(&self, arena: ArenaId, bytes: Option<u64>) {
        self.locked().arenas.set_cap(arena, bytes.map(chunks_in));
        // Waiters may find room now.
        self.room.pulse();
    }

    /// fxdax: caps the memory the arenas may take together, in bytes, rounded down to chunks, or
    /// lifts the cap.
    pub fn set_total_cap(&self, bytes: Option<u64>) {
        self.locked().arenas.set_total_cap(bytes.map(chunks_in));
        self.room.pulse();
    }

    /// fxdax: the cap of the memory the arenas may take together, in bytes, if any.
    pub fn total_cap(&self) -> Option<u64> {
        self.locked().arenas.total_cap().map(chunk_bytes)
    }

    /// fxdax: the memory `arena` has, in bytes: its chunks', free pages and spares included.
    pub fn arena_bytes(&self, arena: ArenaId) -> u64 {
        chunk_bytes(self.locked().arenas.chunks(arena))
    }

    /// fxdax: the memory the arenas have together, in bytes.
    pub fn arenas_bytes(&self) -> u64 {
        chunk_bytes(self.locked().arenas.given())
    }

    /// fxdax: gives the spare chunks of `arena`, or of every arena, wholly free, back to the node:
    /// memory they hold that nothing uses (TASK-50.3).
    pub fn release_spares(&self, arena: Option<ArenaId>) {
        self.locked().arenas.release_spares(&self.pages, arena);
    }

    /// fxdax: sets whether arenas keep spare chunks; those they have go back if not.
    pub fn keep_spare_chunks(&self, keep: bool) {
        self.locked().arenas.keep_spares(&self.pages, keep);
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

        loop {
            let mut locked = self.locked();
            let free_count = self.free_count.load(Ordering::Relaxed);
            if count as u64 > free_count && relief::started() {
                // fxdax: a page at a time, so that the arena, once it has none, is relieved
                // (`relief`), and the allocation waits for that, or fails, as one page's.
                drop(locked);
                let mut taken = VmPageDoublyLinkedList::new();
                for _ in 0..count {
                    match self.alloc_page_in(arena, alloc_flags) {
                        Ok(page) => taken.push_back(page),
                        Err(status) => {
                            // SAFETY: The pages were just allocated, and are this function's.
                            unsafe { self.free_list(&mut taken, PmmOptDelayReuse::Default) };
                            return Err(status);
                        }
                    }
                }
                list.splice(&mut taken);
                return Ok(());
            }
            if count as u64 > free_count {
                // fxdax: told to wait, its callers take the pages a page at a time, each of
                // which, finding memory out, asks eviction for room (`evicts_when_out`,
                // TASK-79.70).
                if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                    && locked.should_wait != ShouldWaitState::Never
                {
                    return Err(Status::SHOULD_WAIT);
                }
                // Allocation failures from the regular free list are likely to become
                // user-visible.
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

            let refused = match self.take_pages_locked(&mut locked, arena, count) {
                Ok(mut alloc_list) => {
                    drop(locked);
                    list.splice(&mut alloc_list);
                    return Ok(());
                }
                Err(refused) => refused,
            };
            self.increment_free_count_locked(&mut locked, count as u64);
            match refused {
                Refused::Starved(starved) => {
                    return Err(self.starved_status(locked, arena, starved, alloc_flags, count));
                }
                Refused::Unmapped => return Err(self.unmapped_status_locked(&mut locked, count)),
                Refused::GoingBack => self.wait_for_chunks_going_back(locked),
            }
        }
    }

    /// fxdax: `count` pages of `arena`'s, taken under the node's lock, whose state `locked` is,
    /// and allocated: every one, or none, as the arena refuses one, those taken then put back.
    /// Each is the one after the last taken, physically, where that one is free, and of its chunk
    /// (`Arenas::take_after`, TASK-108.1).
    fn take_pages_locked(
        &self,
        locked: &mut PmmNodeLocked,
        arena: ArenaId,
        count: usize,
    ) -> Result<VmPageDoublyLinkedList, Refused> {
        let mut taken = VmPageDoublyLinkedList::new();
        let mut last = None;
        for _ in 0..count {
            let page = match last.and_then(|last| locked.arenas.take_after(&self.pages, last)) {
                Some(page) => Ok(page),
                None if count > 1 => locked.arenas.take_run(&self.pages, arena),
                None => locked.arenas.take(&self.pages, arena),
            };
            match page {
                Ok(page) => {
                    // SAFETY: The page was just taken from the free list under the lock.
                    unsafe { Self::alloc_page_helper_locked(page) };
                    taken.push_back(page);
                    last = Some(page);
                }
                Err(refused) => {
                    // Free pages, but too many of other arenas' chunks, or chunks the host maps no
                    // more of, or chunks going back: those taken go back.
                    let mut room = false;
                    while let Some(page) = taken.pop_front() {
                        // SAFETY: The page was allocated above, under the lock, and is this
                        // function's.
                        unsafe { Self::free_page_helper_locked(page) };
                        room |= locked.arenas.put(&self.pages, page, false);
                    }
                    if room {
                        self.room.pulse();
                    }
                    return Err(refused);
                }
            }
        }
        Ok(taken)
    }

    /// Frees a single physical page back to this node.
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid and they are the owner.
    pub unsafe fn free_page(&self, page: VmPagePtr, delay_reuse: PmmOptDelayReuse) {
        // fxdax: unless quarantined, as a second stage may map it still.
        if quarantine::sift_page(page, delay_reuse) {
            return;
        }
        let mut locked = self.locked();
        // pages freed individually shouldn't be in a queue.
        // SAFETY: The caller owns the page.
        debug_assert!(unsafe { !page.in_container() });
        // SAFETY: The caller owns the page.
        unsafe { Self::free_page_helper_locked(page) };
        self.increment_free_count_locked(&mut locked, 1);
        // fxdax: on its arena's free list.
        if locked.arenas.put(&self.pages, page, delay_reuse == PmmOptDelayReuse::Yes) {
            self.room.pulse();
        }
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
        // fxdax: but those quarantined, as a second stage may map them still.
        quarantine::sift(list, delay_reuse);
        self.free_list_locked(self.locked(), list, delay_reuse);
    }

    /// fxdax: counts `list`'s pages, freed, quarantined in their arenas, as relief finds them
    /// (`Arenas::relieved`, TASK-106).
    pub(super) fn quarantining(&self, list: &VmPageDoublyLinkedList) {
        let mut locked = self.locked();
        for page in list.iter() {
            locked.arenas.quarantine(&self.pages, page, true);
        }
    }

    /// fxdax: frees every page on `list`, quarantined, now that every second stage has made the
    /// changes taking them away: as `free_list` does, as their arenas count them back, under the
    /// lock relief finds them under.
    ///
    /// # Safety
    ///
    /// As `free_list`'s; and the pages were quarantined, counted (`quarantining`).
    pub(super) unsafe fn free_quarantined(
        &self,
        list: &mut VmPageDoublyLinkedList,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let mut locked = self.locked();
        for page in list.iter() {
            locked.arenas.quarantine(&self.pages, page, false);
        }
        self.free_list_locked(locked, list, delay_reuse);
    }

    /// Frees every page on `list` back to this node, under the lock, `locked`.
    fn free_list_locked(
        &self,
        mut locked: NodeGuard<'_>,
        list: &mut VmPageDoublyLinkedList,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let mut count = 0;
        for page in list.iter() {
            // SAFETY: The caller owns the pages.
            unsafe { Self::free_page_helper_locked(page) };
            count += 1;
        }
        // fxdax: on their arenas' free lists, in order, as a list spliced to the front of a free
        // list would be.
        let mut pages = Vec::new();
        while let Some(page) = list.pop_front() {
            pages.push(page);
        }
        let delay = delay_reuse == PmmOptDelayReuse::Yes;
        if !delay {
            pages.reverse();
        }
        let mut room = false;
        for page in pages {
            room |= locked.arenas.put(&self.pages, page, delay);
        }
        self.increment_free_count_locked(&mut locked, count);
        if room {
            self.room.pulse();
        }
    }

    /// fxdax: the number of chunks no arena has.
    pub fn count_free_chunks(&self) -> usize {
        self.locked().arenas.free_chunk_count()
    }

    /// Return count of unallocated physical pages in this node.
    pub fn count_free_pages(&self) -> u64 {
        self.free_count.load(Ordering::Relaxed)
    }

    /// Return amount of physical memory in this node, in bytes.
    pub fn count_total_bytes(&self) -> u64 {
        (self.pages.page_array.len() * page::SIZE) as u64
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
        let mut locked = self.locked();
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
    /// fxdax: from `arena`, which, if it is starved while the node does not delay allocations,
    /// waits for room (`PmmNode::room`), and then fails with `SHOULD_WAIT`, for the caller to try
    /// again; or, removed, fails with `NO_MEMORY` at once.
    pub fn wait_for_single_page_allocation_in(
        &self,
        arena: ArenaId,
        deadline: Deadline,
    ) -> Result<VmPagePtr, Status> {
        // If we have been instructed to never wait, skip waiting on the event entirely to
        // prevent blocking. The Never state is final. fxdax: nor does an arena that waits for room
        // of its own wait for the node's memory (TASK-50.3).
        if self.may_wait() && !self.waits_for_room(arena) {
            // Let the caller handle the error and retry if necessary.
            self.may_allocate_evt.wait(&deadline)?;
        }
        // fxdax: what it waits for comes after it tries.
        let room = self.room.count();
        let result = self.allocate_after_wait(arena);
        if result == Err(Status::SHOULD_WAIT) && self.starves(arena) {
            self.room.wait_past(room, &deadline)?;
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
        if self.may_wait() && !self.waits_for_room(arena) {
            self.may_allocate_evt.wait_async(deadline).await?;
        }
        let room = self.room.count();
        let result = self.allocate_after_wait(arena);
        if result == Err(Status::SHOULD_WAIT) && self.starves(arena) {
            self.room.wait_past_async(room, deadline).await?;
        }
        result
    }

    /// fxdax: whether `arena` is what an allocation of it told to wait waits for, rather than the
    /// node: it is starved (`Arenas::starved`), with no free page of its own and no chunk to take,
    /// even with other arenas' spares gone back for one, and the node does not delay allocations.
    /// While it does, the allocation waits at the node's gate as it tries again, as any does
    /// (TASK-86).
    fn starves(&self, arena: ArenaId) -> bool {
        let locked = self.locked();
        locked.should_wait != ShouldWaitState::UntilReset && locked.arenas.starved(arena).is_some()
    }

    /// fxdax: whether an allocation of `arena` waits for room of its own rather than for the
    /// node's memory, at its gate: its arena is starved at a cap, its own or the total's, as things
    /// stand (`Arenas::within_caps`), or starved and relieved, and the node does not delay
    /// allocations. One of an arena at a cap but not starved, its free pages or other arenas'
    /// spares making it room, waits at the gate as any does: told to wait by a node delaying
    /// allocations, it would not wait at all, and its caller would try again at once (TASK-86).
    /// One of an arena removed waits for neither: it fails at once, and leaves the node's gate to
    /// the waiters after it.
    fn waits_for_room(&self, arena: ArenaId) -> bool {
        let locked = self.locked();
        match locked.arenas.starved(arena) {
            Some(Starved::Removed) => true,
            Some(_) => {
                locked.should_wait != ShouldWaitState::UntilReset
                    && (!locked.arenas.within_caps(arena) || relief::started())
            }
            None => false,
        }
    }

    /// Whether an allocation may wait, unless the node has been told never to.
    fn may_wait(&self) -> bool {
        self.locked().should_wait != ShouldWaitState::Never
    }

    /// Attempts an allocation after waiting for the system to exit low memory state.
    fn allocate_after_wait(&self, arena: ArenaId) -> Result<VmPagePtr, Status> {
        // fxdax: a test's, run past the gate as another thread might change the node there.
        #[cfg(test)]
        if let Some(f) = PAST_THE_GATE.take() {
            f();
        }
        // Try to allocate the page now, it may fail sporadically, since there is no guarantee that
        // by the time we attempt to allocate the pages are still available.
        let res = self.alloc_page_in(arena, ALLOC_FLAG_CAN_WAIT);

        // Normally we would only signal in the `ZX_OK` case, i.e. when we are in an allocation-able
        // state. Otherwise we would wake up another thread just for it to receive
        // `ZX_ERR_SHOULD_WAIT` and immediately go back to waiting on the event. However, we also
        // signal when `should_wait == Never`, to ensure all waiting threads are woken up, and in the
        // random wait mode, where we may block in a non low memory state. fxdax: and when the
        // allocation failed though the node has memory, free or for relief to free: its arena's
        // failure, such as its removal's, which the next waiter's, another arena's, need not share.
        let should_signal = match self.locked().should_wait {
            ShouldWaitState::Never => true,
            ShouldWaitState::UntilReset => false,
            ShouldWaitState::OnceLevelTripped => {
                res.is_ok()
                    || res == Err(Status::SHOULD_WAIT)
                    || (rules().arena_failures_pass_the_gate
                        && (self.count_free_pages() > 0 || relief::started()))
            }
        };
        if should_signal {
            self.may_allocate_evt.signal();
        }
        res
    }

    /// Tells the node to stop returning SHOULD_WAIT.
    pub fn stop_returning_should_wait(&self) {
        self.locked().should_wait = ShouldWaitState::Never;
        self.may_allocate_evt.signal();
    }

    /// Returns whether an allocation has failed with NO_MEMORY.
    pub fn has_alloc_failed_no_mem(&self) -> bool {
        self.alloc_failed_no_mem.load(Ordering::Relaxed)
    }

    /// Retrieves information from the first allocation failure.
    pub fn get_first_alloc_failure(&self) -> AllocFailure {
        self.locked().first_alloc_failure
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

    /// fxdax: whether an allocation that finds no free page, of any arena's, asks eviction for
    /// room, as one of an arena starved while other arenas have free pages does
    /// (`starved_status`): with memory never out (`OomBehavior::Evict`), whose watchdog neither
    /// delays allocations nor reclaims memory for them, while eviction runs, and the page queues
    /// hold pages eviction on demand may free (TASK-79.70). Otherwise it fails at once: as
    /// upstream's, whose watchdog delays those that can wait, and reclaims, as memory runs out; or
    /// as nothing would make it room, eviction off, as it is while the scanner is disabled, or
    /// with nothing to evict.
    fn evicts_when_out(&self) -> bool {
        if BootOptions::get().oom_behavior != OomBehavior::Evict
            || !self.evictor.is_eviction_enabled()
        {
            return false;
        }
        let reclaimable = self.page_queues.get_active_inactive_counts();
        reclaimable.active + reclaimable.inactive > 0
    }

    /// fxdax: what an allocation of `size` pages fails with that the host mapped no chunk for
    /// (`Refused::Unmapped`, TASK-79.37): `NO_MEMORY` at once, whether or not it may wait, as
    /// nothing waits for the host; reported as the node's failure, as its memory is out as far as
    /// the host goes.
    fn unmapped_status_locked(&self, locked: &mut PmmNodeLocked, size: usize) -> Status {
        let free_count = self.free_count.load(Ordering::Relaxed);
        let failure = AllocFailure { r#type: AllocFailureType::Pmm, size, free_count };
        self.report_alloc_failure_locked(locked, failure);
        Status::NO_MEMORY
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
        // fxdax: allocated again, it is no second stage's yet.
        page.take_staged();
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

/// fxdax: runs `f` on this thread as its next allocation that waited passes the node's gate,
/// before it tries (`PmmNode::allocate_after_wait`), as another thread might change the node
/// there: a window no test reaches otherwise, between the gate and the node's delay, but by
/// chance (TASK-79.81).
#[cfg(test)]
pub(crate) fn past_the_gate(f: impl FnOnce() + 'static) {
    PAST_THE_GATE.set(Some(Box::new(f)));
}

#[cfg(test)]
thread_local! {
    /// fxdax: what [`past_the_gate`] runs.
    static PAST_THE_GATE: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
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
    let node = node();
    node.locked().arenas.create(&node.pages)
}

/// fxdax: removes `arena`, not the default: now, if none of its pages is allocated; or once the
/// last of them is freed, and its chunk goes back. Its allocations fail with `NO_MEMORY` from now
/// on, whether or not they may wait (TASK-79.36).
pub fn remove_arena(arena: ArenaId) {
    let node = node();
    // Under the lock pages are freed under, so that none goes back between the arena's chunks
    // counted and it retiring.
    node.locked().arenas.remove(&node.pages, arena);
    // Its allocations waiting for room fail now.
    node.room.pulse();
}

/// fxdax: `bytes` of memory, in whole chunks.
fn chunks_in(bytes: u64) -> usize {
    (bytes / CHUNK_SIZE as u64) as usize
}

/// fxdax: the bytes of `chunks` chunks.
fn chunk_bytes(chunks: usize) -> u64 {
    chunks as u64 * CHUNK_SIZE as u64
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
