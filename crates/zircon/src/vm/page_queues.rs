// Copyright 2020 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Queues classifying the pages of VMOs for reclamation.
//!
//! Ported from `zircon/kernel/vm/page_queues.cc` and
//! `zircon/kernel/vm/include/vm/page_queues.h`.
//!
//! Allocated pages that are part of the cow pages in a VmObjectPaged can be placed in a page
//! queue. The page queues provide a way to
//!  * Classify and group pages across VMO boundaries
//!  * Retrieve the VMO that a page is contained in (via a back reference stored in the vm_page_t)
//!
//! Once a page has been placed in a page queue its queue_node becomes owned by the page queue and
//! must not be used until the page has been Remove'd.
//!
//! Reclaimable pages age through a circular buffer of queues indexed by generation, from the most
//! recently used (MRU) to the least recently used (LRU). An MRU thread ages the queues
//! periodically, and an LRU thread moves pages from the oldest queue to the isolate queues, from
//! which the evictor reclaims them.
//!
//! Deviations:
//! - Its threads start through `crate::sync::spawn`, which with the `shuttle` feature starts
//!   none, as time doesn't pass under Shuttle.
//! - A page's backlink to its `VmCowPages` is a `Weak` reference (see `VmCowPages::backlink`), so
//!   that upgrading it cannot race with the `VmCowPages` being destroyed.
//! - No loaned page sweeping, compression or debug compressor, so `LruAction::CompressOnly`
//!   reclaims nothing.
//! - No kcounters or console dump formatting.
//! - fxdax: eviction on demand ages the queues as it finds no page old enough to evict
//!   (`rotate_reclaim_queues`, TASK-79.70).
//! - fxdax: a page's age and backlink can be read by the page (`reclaim_age`, `reclaim_backlink`),
//!   for eviction of an arena's own pages, which finds them by their chunks (TASK-79.76).

use crate::kernel::deadline::{Deadline, current_mono_time};
use crate::kernel::event::{AutounsignalEvent, Event};
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_state::VmPageState;
use crate::vm::scanner;
use crate::vm::vm_cow_pages::{EvictionAction, VmCowPages};
use parking_lot::Mutex;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use zx_types::{ZX_TIME_INFINITE, ZX_TIME_INFINITE_PAST, zx_duration_mono_t, zx_instant_mono_t};

/// The number of reclamation queues. With two active queues 8 page queues are used so that there
/// is some fidelity of information in the inactive queues.
pub const NUM_RECLAIM: usize = 8;
/// Two active queues are used to allow for better fidelity of active information.
pub const NUM_ACTIVE_QUEUES: usize = 2;
/// The amount of pages that will have to move around the queues before the active/inactive ratio
/// is re-checked.
const ACTIVE_INACTIVE_ERROR_MARGIN: u64 = (2 << 20) / page::SIZE as u64;
/// The last two queues are considered the oldest.
pub const NUM_OLDEST_QUEUES: usize = 2;
const _: () = assert!(NUM_RECLAIM > NUM_ACTIVE_QUEUES);
const _: () = assert!(NUM_OLDEST_QUEUES + NUM_ACTIVE_QUEUES <= NUM_RECLAIM);

/// Isolate queues separate "Don't Need" pages (high reclamation priority) from standard aged
/// pages.
const ISOLATE_QUEUE_DONT_NEED: usize = 0;
const ISOLATE_QUEUE_STANDARD: usize = 1;
const NUM_ISOLATE_QUEUES: usize = 2;

pub const DEFAULT_MIN_MRU_ROTATE_TIME: zx_duration_mono_t = 5_000_000_000;
pub const DEFAULT_MAX_MRU_ROTATE_TIME: zx_duration_mono_t = 5_000_000_000;
/// Presently arbitrary, since the min and max mru rotate time are fixed at the same value.
const DEFAULT_ACTIVE_RATIO_MULTIPLIER: u64 = 0;

/// When holding the lock and operating on an arbitrary number of pages, the number of pages after
/// which to check whether the lock should be yielded.
const OP_BATCH_SIZE: usize = 64;

/// The page queue a page is in, stored in the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum PageQueue {
    None = 0,
    Anonymous,
    Wired,
    HighPriority,
    AnonymousZeroFork,
    PagerBackedDirty,
    FailedReclaim,
    ReclaimIsolate,
    ReclaimBase,
}

const RECLAIM_BASE: u8 = PageQueue::ReclaimBase as u8;
const RECLAIM_LAST: u8 = RECLAIM_BASE + NUM_RECLAIM as u8 - 1;
const NUM_QUEUES: usize = RECLAIM_LAST as usize + 1;
const QUEUE_NONE: u8 = PageQueue::None as u8;
const QUEUE_ANONYMOUS: u8 = PageQueue::Anonymous as u8;
const QUEUE_WIRED: u8 = PageQueue::Wired as u8;
const QUEUE_HIGH_PRIORITY: u8 = PageQueue::HighPriority as u8;
const QUEUE_ANONYMOUS_ZERO_FORK: u8 = PageQueue::AnonymousZeroFork as u8;
const QUEUE_PAGER_BACKED_DIRTY: u8 = PageQueue::PagerBackedDirty as u8;
const QUEUE_FAILED_RECLAIM: u8 = PageQueue::FailedReclaim as u8;
const QUEUE_RECLAIM_ISOLATE: u8 = PageQueue::ReclaimIsolate as u8;

// The Isolate queue must be directly before the LRU queues, and the Dirty queue before both.
const _: () = assert!(QUEUE_RECLAIM_ISOLATE + 1 == RECLAIM_BASE);
const _: () = assert!(QUEUE_PAGER_BACKED_DIRTY < QUEUE_RECLAIM_ISOLATE);

/// Converts free running generation to reclaim queue.
const fn gen_to_queue(generation: u64) -> u8 {
    (generation % NUM_RECLAIM as u64) as u8 + RECLAIM_BASE
}

/// Checks if a candidate reclaim page queue would be valid given a specific lru and mru queue.
fn queue_is_valid(page_queue: u8, lru: u8, mru: u8) -> bool {
    debug_assert!(page_queue >= RECLAIM_BASE);
    if lru <= mru {
        page_queue >= lru && page_queue <= mru
    } else {
        page_queue <= mru || page_queue >= lru
    }
}

/// Returns whether this queue is reclaimable, and hence can be active or inactive.
const fn queue_is_reclaim(page_queue: u8) -> bool {
    page_queue >= QUEUE_RECLAIM_ISOLATE
}

/// Calculates the age of a reclaimable queue against a given mru, with 0 meaning page_queue==mru.
fn queue_age(page_queue: u8, mru: u8) -> u32 {
    debug_assert!(page_queue >= RECLAIM_BASE);
    if page_queue <= mru {
        u32::from(mru - page_queue)
    } else {
        // As in Zircon this relies on unsigned wraparound, since the queues are offset from
        // zero by RECLAIM_BASE.
        (NUM_RECLAIM as u32).wrapping_sub(u32::from(page_queue)).wrapping_add(u32::from(mru))
    }
}

fn queue_is_active(page_queue: u8, mru: u8) -> bool {
    page_queue >= RECLAIM_BASE && queue_age(page_queue, mru) < NUM_ACTIVE_QUEUES as u32
}

fn queue_is_inactive(page_queue: u8, mru: u8) -> bool {
    // The Isolate queue does not have an age, but should definitely be considered inactive.
    if page_queue == QUEUE_RECLAIM_ISOLATE {
        return true;
    }
    page_queue >= RECLAIM_BASE && queue_age(page_queue, mru) >= NUM_ACTIVE_QUEUES as u32
}

/// The reason aging was triggered, for debugging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgeReason {
    /// The maximum timeout was reached before any other reason could trigger.
    Timeout,
    /// The allowable ratio of active versus inactive pages was exceeded.
    ActiveRatio,
    /// An explicit call to rotate_reclaim_queues.
    Manual,
}

/// What to do with pages that would otherwise move from the old LRU queue to the isolate queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LruAction {
    None,
    EvictOnly,
    CompressOnly,
    EvictAndCompress,
}

/// Page backlink information acquired whilst holding the page queue lock. The page and offset are
/// not synchronized and must be validated under the cow's lock before use. The cow is `None` if it
/// was being destroyed.
pub struct VmoBacklink {
    pub cow: Option<Arc<VmCowPages>>,
    pub page: VmPagePtr,
    pub offset: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub reclaim: [usize; NUM_RECLAIM],
    pub reclaim_isolate: usize,
    pub pager_backed_dirty: usize,
    pub anonymous: usize,
    pub wired: usize,
    pub anonymous_zero_fork: usize,
    pub failed_reclaim: usize,
    pub high_priority: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveInactiveCounts {
    /// Pages that would normally be available for eviction, but are presently considered active.
    pub active: usize,
    /// Pages that are available for eviction due to not presently being considered active.
    pub inactive: usize,
}

/// Upgrades the backlink stored in a page. Must be called with the list lock held.
fn upgrade_backlink(page: VmPagePtr) -> Option<Arc<VmCowPages>> {
    // SAFETY: The list lock is held, which protects the backlink of a page in the queues.
    let object = unsafe { page.object().get_object() };
    debug_assert!(!object.is_null());
    // SAFETY: A page in the queues holds a weak reference in its backlink; see
    // `VmCowPages::backlink`. It is borrowed here, not consumed.
    let weak = ManuallyDrop::new(unsafe { Weak::from_raw(object.cast::<VmCowPages>()) });
    weak.upgrade()
}

/// Releases the weak reference held by a page's backlink.
///
/// # Safety
///
/// `object` must be a backlink from `VmCowPages::backlink` that is no longer stored in a page.
unsafe fn release_backlink(object: *mut ()) {
    // SAFETY: Guaranteed by the caller.
    drop(unsafe { Weak::from_raw(object.cast::<VmCowPages>()) });
}

struct Lists {
    /// The page queues, indexed by page queue. The list for PageQueue::None is always empty.
    page_queues: [VmPageDoublyLinkedList; NUM_QUEUES],
    /// Pages in the PageQueue::ReclaimIsolate state are in exactly one of these lists.
    isolate_queues: [VmPageDoublyLinkedList; NUM_ISOLATE_QUEUES],
}

struct Locked {
    /// Externally supplied event that we should signal anytime aging occurs.
    aging_event: Option<Arc<Event>>,
    /// Whether the active aging via the mru thread should be disabled.
    aging_disabled: bool,
    /// Reason the last aging event happened, for debugging.
    last_age_reason: AgeReason,
    /// Tracks whether the active ratio has been tripped and should contribute as an aging trigger.
    active_ratio_triggered: bool,
    /// What to do with pages when processing the LRU queue.
    lru_action: LruAction,
    mru_thread: Option<JoinHandle<()>>,
    lru_thread: Option<JoinHandle<()>>,
    active_ratio_multiplier: u64,
}

pub struct PageQueues {
    /// Protects the queue lists, so that latency sensitive operations such as adding and removing
    /// pages don't contend with other page queue operations. Where both locks need to be acquired,
    /// `lock` must be acquired prior to `list_lock`.
    list_lock: Mutex<Lists>,
    /// General lock used to protect all logic and members that are not part of critical latency
    /// sensitive operations.
    lock: Mutex<Locked>,
    /// Time at which the mru_gen was last incremented.
    last_age_time: AtomicI64,
    /// Signaled when the mru thread should check if the mru generation needs incrementing.
    mru_event: AutounsignalEvent,
    /// Signaled when the lru thread should check if the lru queue needs processing.
    lru_event: AutounsignalEvent,
    /// The generation counts are monotonic increasing counters representing the effective age of
    /// the oldest and newest reclaimable queues. They are only modified with the lock held.
    lru_gen: AtomicU64,
    mru_gen: AtomicU64,
    /// Counts of pages in each queue. The count for None is unused.
    page_queue_counts: [AtomicUsize; NUM_QUEUES],
    /// Count of how many pages have moved queue without us recalculating the active ratio.
    lazy_active_ratio_aging_skips: AtomicU64,
    shutdown_threads: AtomicBool,
    min_mru_rotate_time: AtomicI64,
    max_mru_rotate_time: AtomicI64,
    /// Whether anonymous zero page forks are placed in the reclaimable queues.
    zero_fork_is_reclaimable: AtomicBool,
    /// Whether anonymous pages are placed in the reclaimable queues.
    anonymous_is_reclaimable: AtomicBool,
}

/// Collects pages to reclaim while the page queue locks are held, reclaiming them once they are
/// dropped, as `LruIsolate` in page_queues.cc does.
struct LruIsolate {
    lru_action: LruAction,
    list: Vec<(VmoBacklink, bool)>,
}

impl LruIsolate {
    const ITEMS: usize = 16;

    fn new() -> Self {
        Self { lru_action: LruAction::None, list: Vec::with_capacity(Self::ITEMS) }
    }

    /// Adds a page to be reclaimed if the lru action applies to it. Requires the list lock.
    fn add_reclaimable(&mut self, page: VmPagePtr) {
        if self.lru_action == LruAction::None {
            return;
        }
        let Some(cow) = upgrade_backlink(page) else { return };
        // SAFETY: The list lock is held.
        let offset = unsafe { page.object().get_page_offset() };
        let reclaim = self.lru_action == LruAction::EvictAndCompress
            || ((cow.can_evict() || cow.is_discardable())
                == (self.lru_action == LruAction::EvictOnly));
        // Even if not reclaiming, keep the reference so it is dropped outside the lock.
        self.list.push((VmoBacklink { cow: Some(cow), page, offset }, reclaim));
    }

    fn full(&self) -> bool {
        self.list.len() == Self::ITEMS
    }

    /// Reclaims the stored pages. Requires the page queue locks not be held.
    fn flush(&mut self) {
        for (backlink, reclaim) in self.list.drain(..) {
            if reclaim && let Some(cow) = backlink.cow {
                let _ =
                    cow.reclaim_page(backlink.page, backlink.offset, EvictionAction::FollowHint);
            }
        }
    }
}

impl Drop for LruIsolate {
    fn drop(&mut self) {
        self.flush();
    }
}

impl Default for PageQueues {
    fn default() -> Self {
        Self::new()
    }
}

impl PageQueues {
    pub fn new() -> Self {
        Self {
            list_lock: Mutex::new(Lists {
                page_queues: Default::default(),
                isolate_queues: Default::default(),
            }),
            lock: Mutex::new(Locked {
                aging_event: None,
                aging_disabled: false,
                last_age_reason: AgeReason::Timeout,
                active_ratio_triggered: false,
                lru_action: LruAction::None,
                mru_thread: None,
                lru_thread: None,
                active_ratio_multiplier: DEFAULT_ACTIVE_RATIO_MULTIPLIER,
            }),
            last_age_time: AtomicI64::new(ZX_TIME_INFINITE_PAST),
            mru_event: AutounsignalEvent::new(false),
            lru_event: AutounsignalEvent::new(false),
            lru_gen: AtomicU64::new(0),
            mru_gen: AtomicU64::new(NUM_RECLAIM as u64 - 1),
            page_queue_counts: Default::default(),
            lazy_active_ratio_aging_skips: AtomicU64::new(0),
            shutdown_threads: AtomicBool::new(false),
            min_mru_rotate_time: AtomicI64::new(DEFAULT_MIN_MRU_ROTATE_TIME),
            max_mru_rotate_time: AtomicI64::new(DEFAULT_MAX_MRU_ROTATE_TIME),
            zero_fork_is_reclaimable: AtomicBool::new(false),
            anonymous_is_reclaimable: AtomicBool::new(false),
        }
    }

    fn mru_gen_to_queue(&self) -> u8 {
        gen_to_queue(self.mru_gen.load(Ordering::Relaxed))
    }

    fn count(&self, queue: u8) -> usize {
        self.page_queue_counts[queue as usize].load(Ordering::Relaxed)
    }

    /// Creates the threads for queue management.
    pub fn start_threads(
        &'static self,
        min_mru_rotate_time: zx_duration_mono_t,
        max_mru_rotate_time: zx_duration_mono_t,
    ) {
        // Clamp the max rotate to the minimum, and prevent a rotation rate that is too small.
        let max_mru_rotate_time = max_mru_rotate_time.max(min_mru_rotate_time).max(1_000_000_000);
        self.min_mru_rotate_time.store(min_mru_rotate_time, Ordering::Relaxed);
        self.max_mru_rotate_time.store(max_mru_rotate_time, Ordering::Relaxed);

        let mru_thread = crate::sync::spawn("page-queue-mru-thread", move || self.mru_thread());
        let lru_thread = crate::sync::spawn("page-queue-lru-thread", move || self.lru_thread());
        let mut locked = self.lock.lock();
        assert!(locked.mru_thread.is_none() && locked.lru_thread.is_none());
        locked.mru_thread = mru_thread;
        locked.lru_thread = lru_thread;
        // Kick start any LRU processing that might be pending to ensure it doesn't spuriously
        // timeout.
        self.maybe_trigger_lru_processing_locked();
    }

    pub fn set_lru_action(&self, action: LruAction) {
        self.lock.lock().lru_action = action;
    }

    pub fn set_active_ratio_multiplier(&self, multiplier: u32) {
        let mut locked = self.lock.lock();
        locked.active_ratio_multiplier = u64::from(multiplier);
        // The change in multiplier might have caused us to need to age.
        self.check_active_ratio_aging_locked(&mut locked);
    }

    fn check_active_ratio_aging_locked(&self, locked: &mut Locked) {
        if locked.active_ratio_triggered {
            // Already triggered, nothing more to do.
            return;
        }
        let counts = self.get_active_inactive_counts();
        if counts.active as u64 * locked.active_ratio_multiplier > counts.inactive as u64 {
            locked.active_ratio_triggered = true;
            self.mru_event.signal();
        }
    }

    /// Synchronizes with any outstanding aging. This allows a reclamation process to ensure it is
    /// not racing with, and falsely failing to reclaim, the aging thread due to scheduling or
    /// other delays.
    fn synchronize_with_aging(&self) {
        let mut iterations = 1;
        loop {
            // Typically this loop should exit on its second iteration at most, so many iterations
            // suggest a bug.
            if iterations % 20 == 0 {
                eprintln!("[pq]: Warning synchronize_with_aging iterated {iterations} times");
            }
            iterations += 1;
            let locked = self.lock.lock();
            // If the LRU queue can be processed, then aging already happened.
            if self.can_increment_lru_gen_locked() {
                return;
            }
            // Check for races with LRU processing that might have already populated the isolate
            // list.
            if self.count(QUEUE_RECLAIM_ISOLATE) > 0 {
                return;
            }
            // If there's no pending age reason then we are not going to wait, so return.
            let reason = match self.get_age_reason_locked(&locked) {
                Ok(reason) => reason,
                Err(_) => return,
            };
            // Attempt to perform any aging, and then go around the loop to check if we
            // synchronized.
            self.try_aging_locked(reason, locked);
        }
    }

    /// Returns the pending age reason, or when to check again.
    fn get_age_reason_locked(&self, locked: &Locked) -> Result<AgeReason, zx_instant_mono_t> {
        let current = current_mono_time();
        let last_age_time = self.last_age_time.load(Ordering::Relaxed);
        if locked.active_ratio_triggered {
            // Need to have passed the min time though.
            let min_timeout =
                last_age_time.saturating_add(self.min_mru_rotate_time.load(Ordering::Relaxed));
            if current < min_timeout {
                return Err(min_timeout);
            }
            return Ok(AgeReason::ActiveRatio);
        }
        // Exceeding the maximum time forces aging.
        let max_timeout =
            last_age_time.saturating_add(self.max_mru_rotate_time.load(Ordering::Relaxed));
        if max_timeout <= current {
            return Ok(AgeReason::Timeout);
        }
        // With no other reason, we will age once we hit the maximum timeout.
        Err(max_timeout)
    }

    fn maybe_trigger_lru_processing_locked(&self) {
        if self.needs_lru_processing_locked() {
            self.lru_event.signal();
        }
    }

    fn needs_lru_processing_locked(&self) -> bool {
        // Currently only reason to trigger lru processing is if the MRU needs space.
        self.mru_gen.load(Ordering::Relaxed) - self.lru_gen.load(Ordering::Relaxed)
            == NUM_RECLAIM as u64 - 1
    }

    /// Returns whether it is permissible to increase the mru generation.
    fn can_increment_mru_gen_locked(&self) -> bool {
        self.mru_gen.load(Ordering::Relaxed) - self.lru_gen.load(Ordering::Relaxed)
            < NUM_RECLAIM as u64 - 1
    }

    /// Similar to `can_increment_mru_gen_locked`, but for the lru.
    fn can_increment_lru_gen_locked(&self) -> bool {
        self.mru_gen.load(Ordering::Relaxed) - self.lru_gen.load(Ordering::Relaxed)
            > NUM_ACTIVE_QUEUES as u64
    }

    pub fn disable_aging(&self) {
        let mut locked = self.lock.lock();
        if locked.aging_disabled {
            panic!("Mismatched disable/enable pair");
        }
        locked.aging_disabled = true;
    }

    pub fn enable_aging(&self) {
        let mut locked = self.lock.lock();
        if !locked.aging_disabled {
            panic!("Mismatched disable/enable pair");
        }
        locked.aging_disabled = false;
        // Notify the threads, allowing them to proceed with any pending actions.
        self.mru_event.signal();
        self.lru_event.signal();
    }

    /// Registers an event that will be signaled every time aging occurs.
    pub fn set_aging_event(&self, event: Option<Arc<Event>>) {
        let mut locked = self.lock.lock();
        assert!(event.is_none() || locked.aging_event.is_none());
        locked.aging_event = event;
    }

    /// Runs the aging thread. Aging requires very little work and is more about coordination.
    fn mru_thread(&self) {
        // Pretend that aging happens during startup to simplify the rest of the loop logic.
        self.last_age_time.store(current_mono_time(), Ordering::Relaxed);
        while !self.shutdown_threads.load(Ordering::Relaxed) {
            // Attempt to perform aging steps. Once there is no more aging to be done this returns
            // a deadline to wait for.
            let wait_deadline = loop {
                if self.shutdown_threads.load(Ordering::Relaxed) {
                    break ZX_TIME_INFINITE_PAST;
                }
                let locked = self.lock.lock();
                // If aging is disabled or we are unable to age then wait to be signaled.
                if locked.aging_disabled || !self.can_increment_mru_gen_locked() {
                    break ZX_TIME_INFINITE;
                }
                match self.get_age_reason_locked(&locked) {
                    // Cannot age yet, so wait till the provided deadline or we are signaled.
                    Err(deadline) => break deadline,
                    Ok(reason) => self.try_aging_locked(reason, locked),
                }
            };
            // Wait on the mru_event instead of just sleeping, as it will get signaled should
            // anything change that may cause us to be able to age sooner.
            let _ = self.mru_event.wait(&Deadline::no_slack(wait_deadline));
        }
    }

    /// Processes the LRU queue when the MRU needs space.
    fn lru_thread(&self) {
        const LRU_NEEDS_PROCESSING_POLL_SECONDS: i64 = 90;
        let mut pending_target_gen = u64::MAX;
        while !self.shutdown_threads.load(Ordering::Relaxed) {
            let wait_status = self
                .lru_event
                .wait(&Deadline::after_mono(LRU_NEEDS_PROCESSING_POLL_SECONDS * 1_000_000_000));
            // Take the lock so we can calculate (race free) a target lru gen.
            let (mut needs_processing, mut target_gen) = {
                let _locked = self.lock.lock();
                (self.needs_lru_processing_locked(), self.lru_gen.load(Ordering::Relaxed) + 1)
            };
            if !needs_processing {
                continue;
            }
            if wait_status == Err(zx_status::Status::TIMED_OUT) {
                // The queue needs processing, but we woke up due to a timeout on the event and not
                // a signal. This could happen due to a race, so record the target_gen and wait for
                // the signal. If this happens twice for the same target, the signal was missed.
                if pending_target_gen != target_gen {
                    pending_target_gen = target_gen;
                    continue;
                }
                eprintln!(
                    "ERROR LruThread signal was not seen after {LRU_NEEDS_PROCESSING_POLL_SECONDS} \
                     seconds and queue needs processing"
                );
            }
            // Keep processing until we have caught up to what is required.
            while needs_processing {
                // With the lock dropped process the target. This is not racy as generations are
                // monotonic.
                self.process_lru_queue(target_gen, None);
                let _locked = self.lock.lock();
                needs_processing = self.needs_lru_processing_locked();
                target_gen = self.lru_gen.load(Ordering::Relaxed) + 1;
            }
        }
    }

    /// Attempts to perform aging with the given reason, releasing the lock.
    fn try_aging_locked(
        &self,
        age_reason: AgeReason,
        mut locked: parking_lot::MutexGuard<'_, Locked>,
    ) {
        let last_age_time = self.last_age_time.load(Ordering::Relaxed);
        if scanner::scanner_needs_accessed_scan(last_age_time) {
            // Perform the accessed scan without our lock held to avoid unnecessary contention.
            drop(locked);
            scanner::scanner_wait_for_accessed_scan(last_age_time);
        } else {
            self.increment_mru_gen_locked(&mut locked, age_reason);
        }
    }

    fn increment_mru_gen_locked(&self, locked: &mut Locked, age_reason: AgeReason) {
        assert!(self.can_increment_mru_gen_locked());
        // Increment the mru generation and record the current age reason etc.
        self.mru_gen.fetch_add(1, Ordering::Relaxed);
        self.last_age_time.store(current_mono_time(), Ordering::Relaxed);
        locked.last_age_reason = age_reason;
        // As aging has happened we should consume any old active ratio trigger and re-calculate it.
        locked.active_ratio_triggered = false;
        self.check_active_ratio_aging_locked(locked);
        // Signal any externally supplied aging event if one exists.
        if let Some(event) = &locked.aging_event {
            event.signal();
        }
        // Required to notify the LRU thread of mru generation changes.
        self.maybe_trigger_lru_processing_locked();
    }

    /// Performs a manually requested aging event. Only for tests and debugging; fxdax: and for
    /// eviction on demand (`EvictionLevel::OnDemand`), of pages too new for aging to have made them
    /// old enough to evict, as an allocation finds memory out (TASK-79.70).
    pub fn rotate_reclaim_queues(&self) {
        let mut locked = self.lock.lock();
        // 'Force' aging to happen by processing the lru queue until we are able to increment the
        // mru gen.
        while !self.can_increment_mru_gen_locked() {
            let target_gen = self.lru_gen.load(Ordering::Relaxed) + 1;
            parking_lot::MutexGuard::unlocked(&mut locked, || {
                self.process_lru_queue(target_gen, None)
            });
        }
        self.increment_mru_gen_locked(&mut locked, AgeReason::Manual);
    }

    fn peek_isolate_list(&self) -> Option<VmoBacklink> {
        let lists = self.list_lock.lock();
        for list in &lists.isolate_queues {
            if let Some(head) = list.front() {
                debug_assert_eq!(head.page_queue().load(Ordering::Relaxed), QUEUE_RECLAIM_ISOLATE);
                let cow = upgrade_backlink(head);
                // SAFETY: The list lock is held.
                let offset = unsafe { head.object().get_page_offset() };
                drop(lists);
                return Some(VmoBacklink { cow, page: head, offset });
            }
        }
        None
    }

    /// Processes the LRU queue with the goal of increasing the lru_gen to the target_gen. Pages
    /// that have been accessed are moved to their correct queue, the others are moved to the
    /// isolate queue. An optional limit for the number of pages to isolate can be provided.
    fn process_lru_queue(&self, target_gen: u64, isolate: Option<usize>) {
        assert!(
            target_gen <= self.mru_gen.load(Ordering::Relaxed) - (NUM_ACTIVE_QUEUES as u64 - 1)
        );
        let active_inactive = self.get_active_inactive_counts();
        let max_lru_iterations =
            (active_inactive.active + active_inactive.inactive + NUM_RECLAIM) as u64;
        let mut loop_iterations = 0u64;
        // Pages to reclaim are accumulated and acted on after the lock is released.
        let mut deferred_list = LruIsolate::new();
        let mut isolate_remaining = isolate.unwrap_or(usize::MAX);

        while isolate_remaining > 0 {
            if loop_iterations == max_lru_iterations {
                eprintln!(
                    "[pq]: WARNING: process_lru_queue exceeded expected max LRU loop iterations \
                     {max_lru_iterations}"
                );
            }
            loop_iterations += 1;

            deferred_list.flush();
            let mut post = false;
            {
                // Need to hold the general lock, in addition to the list lock, so that the lru_gen
                // does not change while we are working.
                let locked = self.lock.lock();
                deferred_list.lru_action = locked.lru_action;
                let mut lists = self.list_lock.lock();
                let lru = self.lru_gen.load(Ordering::Relaxed);
                if lru >= target_gen {
                    break;
                }
                let mru_queue = self.mru_gen_to_queue();
                let lru_queue = gen_to_queue(lru);
                while !lists.page_queues[lru_queue as usize].is_empty()
                    && !deferred_list.full()
                    && isolate_remaining > 0
                {
                    // Newer pages accumulate at the head of the list, while older pages are at the
                    // tail. Pages are removed from the tail to ensure FIFO processing.
                    let page = lists.page_queues[lru_queue as usize].pop_back().unwrap();
                    let page_queue = page.page_queue().load(Ordering::Relaxed);
                    debug_assert!(page_queue >= RECLAIM_BASE);

                    // If the queue stored in the page does not match then we want to move it to
                    // its correct queue with the caveat that its queue could be invalid, if
                    // mark_accessed had raced, in which case the page is actually very old.
                    if page_queue != lru_queue && queue_is_valid(page_queue, lru_queue, mru_queue) {
                        // Pages are added to the head because they are newer pages.
                        Self::push_front(&mut lists, page_queue, page);
                    } else {
                        // Force it to the isolate list, don't care about races.
                        let old_queue =
                            page.page_queue().swap(QUEUE_RECLAIM_ISOLATE, Ordering::Relaxed);
                        debug_assert!(old_queue >= RECLAIM_BASE);
                        self.page_queue_counts[old_queue as usize].fetch_sub(1, Ordering::Relaxed);
                        self.page_queue_counts[QUEUE_RECLAIM_ISOLATE as usize]
                            .fetch_add(1, Ordering::Relaxed);
                        // Pages are added to the tail of the isolate queue to preserve relative
                        // age (older pages at the head, newer at the tail).
                        Self::push_back_isolate(&mut lists, ISOLATE_QUEUE_STANDARD, page);
                        deferred_list.add_reclaimable(page);
                        isolate_remaining -= 1;
                    }
                }
                if lists.page_queues[lru_queue as usize].is_empty() {
                    // lru_gen is always modified with the lock held, so this precisely sets it to
                    // lru + 1.
                    let prev = self.lru_gen.fetch_add(1, Ordering::Relaxed);
                    debug_assert_eq!(prev, lru);
                    post = true;
                }
                drop(lists);
                drop(locked);
            }
            if post {
                // The lru gen was changed and the mru thread might be waiting for space to
                // increment the mru gen, so kick it.
                self.mru_event.signal();
            }
        }
    }

    fn mark_accessed_maybe_isolate(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            let old_gen = page.page_queue().load(Ordering::Relaxed);
            // With the lock held check if the page is in a reclaim queue. Although it can move
            // between different reclaim queues, it cannot go from reclaim->non-reclaim without the
            // lock.
            if !queue_is_reclaim(old_gen) {
                return;
            }
            self.move_to_queue_locked_list(&mut lists, page, self.mru_gen_to_queue());
        }
        self.maybe_check_active_ratio_aging(1);
    }

    /// Tells the page queue this page has been accessed, and it should have its position in the
    /// queues updated.
    pub fn mark_accessed(&self, page: VmPagePtr) {
        let queue_ref = page.page_queue();
        let mut old_gen = queue_ref.load(Ordering::Relaxed);
        let target_queue = self.mru_gen_to_queue();
        if old_gen == target_queue {
            return;
        }
        loop {
            // If old_gen is not in the active/inactive range then the page has either been racily
            // removed from, or was never in, the reclaim queue.
            if !queue_is_reclaim(old_gen) {
                return;
            }
            // The Isolate queue is a reclaim queue, but we must actually move the page between the
            // lists.
            if old_gen == QUEUE_RECLAIM_ISOLATE {
                self.mark_accessed_maybe_isolate(page);
                return;
            }
            match queue_ref.compare_exchange_weak(
                old_gen,
                target_queue,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => old_gen = actual,
            }
        }
        self.page_queue_counts[old_gen as usize].fetch_sub(1, Ordering::Relaxed);
        self.page_queue_counts[target_queue as usize].fetch_add(1, Ordering::Relaxed);
        self.maybe_check_active_ratio_aging(1);
    }

    /// Records that `pages` have potentially changed queue, returning true if checking the active
    /// ratio can be skipped.
    fn record_active_ratio_skips(&self, pages: u64) -> bool {
        // Check if our specific addition caused the count to cross the threshold, preventing a
        // thundering herd of threads all noticing once the count passes the threshold.
        let old_count = self.lazy_active_ratio_aging_skips.fetch_add(pages, Ordering::Relaxed);
        if old_count < ACTIVE_INACTIVE_ERROR_MARGIN
            && old_count + pages >= ACTIVE_INACTIVE_ERROR_MARGIN
        {
            self.lazy_active_ratio_aging_skips.store(0, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn maybe_check_active_ratio_aging(&self, pages: u64) {
        if !self.record_active_ratio_skips(pages) {
            let mut locked = self.lock.lock();
            self.check_active_ratio_aging_locked(&mut locked);
        }
    }

    fn set_queue_backlink_locked_list(
        &self,
        lists: &mut Lists,
        page: VmPagePtr,
        object: *mut (),
        page_offset: u64,
        queue: u8,
    ) {
        debug_assert_ne!(queue, QUEUE_RECLAIM_ISOLATE);
        debug_assert_eq!(page.state(), VmPageState::Object);
        debug_assert!(!object.is_null());
        // SAFETY: The list lock is held and the caller owns the page.
        unsafe {
            debug_assert!(!page.in_container());
            let state = page.object();
            debug_assert!(state.get_object().is_null());
            debug_assert_eq!(state.get_page_offset(), 0);
            state.set_object(object);
            state.set_page_offset(page_offset);
        }
        debug_assert_eq!(page.page_queue().load(Ordering::Relaxed), QUEUE_NONE);
        page.page_queue().store(queue, Ordering::Relaxed);
        Self::push_front(lists, queue, page);
        self.page_queue_counts[queue as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn move_to_queue_locked_list(&self, lists: &mut Lists, page: VmPagePtr, queue: u8) {
        debug_assert_ne!(queue, QUEUE_RECLAIM_ISOLATE);
        debug_assert_eq!(page.state(), VmPageState::Object);
        let old_queue = page.page_queue().swap(queue, Ordering::Relaxed);
        debug_assert_ne!(old_queue, QUEUE_NONE);
        Self::list_of(lists, page).erase(page);
        Self::push_front(lists, queue, page);
        self.page_queue_counts[old_queue as usize].fetch_sub(1, Ordering::Relaxed);
        self.page_queue_counts[queue as usize].fetch_add(1, Ordering::Relaxed);
    }

    fn move_to_isolate_locked_list(&self, lists: &mut Lists, page: VmPagePtr, index: usize) {
        debug_assert!(index < NUM_ISOLATE_QUEUES);
        debug_assert_eq!(page.state(), VmPageState::Object);
        let old_queue = page.page_queue().swap(QUEUE_RECLAIM_ISOLATE, Ordering::Relaxed);
        debug_assert_ne!(old_queue, QUEUE_NONE);
        Self::list_of(lists, page).erase(page);
        Self::push_back_isolate(lists, index, page);
        self.page_queue_counts[old_queue as usize].fetch_sub(1, Ordering::Relaxed);
        self.page_queue_counts[QUEUE_RECLAIM_ISOLATE as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the list a page is linked into, from the tag set when it was added.
    fn list_of(lists: &mut Lists, page: VmPagePtr) -> &mut VmPageDoublyLinkedList {
        // SAFETY: The list lock is held.
        let tag = unsafe { page.list_tag() } as usize;
        if tag < NUM_QUEUES {
            &mut lists.page_queues[tag]
        } else {
            &mut lists.isolate_queues[tag - NUM_QUEUES]
        }
    }

    /// Adds a page to the front of the list for `queue`.
    fn push_front(lists: &mut Lists, queue: u8, page: VmPagePtr) {
        lists.page_queues[queue as usize].push_front(page);
        // SAFETY: The list lock is held and the page was just added.
        unsafe { page.set_list_tag(queue) };
    }

    /// Adds a page to the back of an isolate list.
    fn push_back_isolate(lists: &mut Lists, index: usize, page: VmPagePtr) {
        lists.isolate_queues[index].push_back(page);
        // SAFETY: The list lock is held and the page was just added.
        unsafe { page.set_list_tag((NUM_QUEUES + index) as u8) };
    }

    pub fn set_wired(&self, page: VmPagePtr, object: *mut (), page_offset: u64) {
        let mut lists = self.list_lock.lock();
        self.set_queue_backlink_locked_list(&mut lists, page, object, page_offset, QUEUE_WIRED);
    }

    pub fn move_to_wired(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            self.move_to_queue_locked_list(&mut lists, page, QUEUE_WIRED);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    fn anonymous_queue(&self, skip_reclaim: bool) -> u8 {
        if self.anonymous_is_reclaimable.load(Ordering::Relaxed) && !skip_reclaim {
            self.mru_gen_to_queue()
        } else {
            QUEUE_ANONYMOUS
        }
    }

    pub fn set_anonymous(
        &self,
        page: VmPagePtr,
        object: *mut (),
        page_offset: u64,
        skip_reclaim: bool,
    ) {
        {
            let mut lists = self.list_lock.lock();
            let queue = self.anonymous_queue(skip_reclaim);
            self.set_queue_backlink_locked_list(&mut lists, page, object, page_offset, queue);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn set_high_priority(&self, page: VmPagePtr, object: *mut (), page_offset: u64) {
        let mut lists = self.list_lock.lock();
        self.set_queue_backlink_locked_list(
            &mut lists,
            page,
            object,
            page_offset,
            QUEUE_HIGH_PRIORITY,
        );
    }

    pub fn move_to_high_priority(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            self.move_to_queue_locked_list(&mut lists, page, QUEUE_HIGH_PRIORITY);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn move_to_anonymous(&self, page: VmPagePtr, skip_reclaim: bool) {
        {
            let mut lists = self.list_lock.lock();
            let queue = self.anonymous_queue(skip_reclaim);
            self.move_to_queue_locked_list(&mut lists, page, queue);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn set_reclaim(&self, page: VmPagePtr, object: *mut (), page_offset: u64) {
        {
            let mut lists = self.list_lock.lock();
            let queue = self.mru_gen_to_queue();
            self.set_queue_backlink_locked_list(&mut lists, page, object, page_offset, queue);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn move_to_reclaim(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            let queue = self.mru_gen_to_queue();
            self.move_to_queue_locked_list(&mut lists, page, queue);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn move_to_reclaim_dont_need(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            self.move_to_isolate_locked_list(&mut lists, page, ISOLATE_QUEUE_DONT_NEED);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    pub fn set_pager_backed_dirty(&self, page: VmPagePtr, object: *mut (), page_offset: u64) {
        let mut lists = self.list_lock.lock();
        self.set_queue_backlink_locked_list(
            &mut lists,
            page,
            object,
            page_offset,
            QUEUE_PAGER_BACKED_DIRTY,
        );
    }

    pub fn move_to_pager_backed_dirty(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            self.move_to_queue_locked_list(&mut lists, page, QUEUE_PAGER_BACKED_DIRTY);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    fn zero_fork_queue(&self) -> u8 {
        if self.zero_fork_is_reclaimable.load(Ordering::Relaxed) {
            self.mru_gen_to_queue()
        } else {
            QUEUE_ANONYMOUS_ZERO_FORK
        }
    }

    /// If a page is presently in the anonymous queue moves it to the anonymous zero fork queue.
    pub fn move_anonymous_to_anonymous_zero_fork(&self, page: VmPagePtr) {
        let anonymous_is_reclaimable = self.anonymous_is_reclaimable.load(Ordering::Relaxed);
        // Short-circuit the case where the page is already in the anonymous queue and both the
        // anonymous and zero fork queues are the same reclaimable queue.
        if self.zero_fork_is_reclaimable.load(Ordering::Relaxed)
            && anonymous_is_reclaimable
            && queue_is_reclaim(page.page_queue().load(Ordering::Relaxed))
        {
            return;
        }
        {
            let mut lists = self.list_lock.lock();
            let queue = page.page_queue().load(Ordering::Relaxed);
            if anonymous_is_reclaimable && !queue_is_reclaim(queue) {
                return;
            }
            if !anonymous_is_reclaimable && queue != QUEUE_ANONYMOUS {
                return;
            }
            let target = self.zero_fork_queue();
            self.move_to_queue_locked_list(&mut lists, page, target);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    /// Moves a page that failed compression out of the reclaim set.
    pub fn compress_failed(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            if queue_is_reclaim(page.page_queue().load(Ordering::Relaxed)) {
                self.move_to_queue_locked_list(&mut lists, page, QUEUE_FAILED_RECLAIM);
            }
        }
        self.maybe_check_active_ratio_aging(1);
    }

    fn remove_locked_list(&self, lists: &mut Lists, page: VmPagePtr) {
        // Directly exchange the old gen.
        let old_queue = page.page_queue().swap(QUEUE_NONE, Ordering::Relaxed);
        debug_assert_ne!(old_queue, QUEUE_NONE);
        self.page_queue_counts[old_queue as usize].fetch_sub(1, Ordering::Relaxed);
        Self::list_of(lists, page).erase(page);
        // SAFETY: The list lock is held, and the backlink is being removed from the page.
        unsafe {
            let object = page.object().get_object();
            page.object().set_object(std::ptr::null_mut());
            page.object().set_page_offset(0);
            release_backlink(object);
        }
    }

    /// Removes the page from any page list and returns ownership of the queue_node.
    pub fn remove(&self, page: VmPagePtr) {
        {
            let mut lists = self.list_lock.lock();
            self.remove_locked_list(&mut lists, page);
        }
        self.maybe_check_active_ratio_aging(1);
    }

    /// Batched version of `remove` that also places all the pages in `out_list`.
    pub fn remove_array_into_list(
        &self,
        pages: &[VmPagePtr],
        out_list: &mut VmPageDoublyLinkedList,
    ) {
        for chunk in pages.chunks(OP_BATCH_SIZE) {
            let mut lists = self.list_lock.lock();
            for &page in chunk {
                self.remove_locked_list(&mut lists, page);
                out_list.push_back(page);
            }
        }
        self.maybe_check_active_ratio_aging(pages.len() as u64);
    }

    pub fn queue_counts(&self) -> Counts {
        let mut counts = Counts::default();
        let _lists = self.list_lock.lock();
        let lru = self.lru_gen.load(Ordering::Relaxed);
        let mru = self.mru_gen.load(Ordering::Relaxed);
        for index in lru..=mru {
            counts.reclaim[(mru - index) as usize] = self.count(gen_to_queue(index));
        }
        counts.reclaim_isolate = self.count(QUEUE_RECLAIM_ISOLATE);
        counts.pager_backed_dirty = self.count(QUEUE_PAGER_BACKED_DIRTY);
        counts.anonymous = self.count(QUEUE_ANONYMOUS);
        counts.wired = self.count(QUEUE_WIRED);
        counts.anonymous_zero_fork = self.count(QUEUE_ANONYMOUS_ZERO_FORK);
        counts.failed_reclaim = self.count(QUEUE_FAILED_RECLAIM);
        counts.high_priority = self.count(QUEUE_HIGH_PRIORITY);
        counts
    }

    pub fn debug_page_is_pager_backed_dirty(&self, page: VmPagePtr) -> bool {
        page.page_queue().load(Ordering::Relaxed) == QUEUE_PAGER_BACKED_DIRTY
    }

    /// fxdax: how old `page` is, if it is in a reclaim queue: older than any queue's, isolated, or
    /// else its queue's age, 0 the newest's. Read without the lock, it may be out of date at once,
    /// but serves eviction of an arena's own pages, which finds them by their chunks rather than in
    /// the queues, to take the oldest first (`Evictor::evict_arena_on_demand`, TASK-79.76).
    pub fn reclaim_age(&self, page: VmPagePtr) -> Option<u32> {
        match page.page_queue().load(Ordering::Relaxed) {
            QUEUE_RECLAIM_ISOLATE => Some(NUM_RECLAIM as u32),
            queue if queue >= RECLAIM_BASE => Some(queue_age(queue, self.mru_gen_to_queue())),
            _ => None,
        }
    }

    /// fxdax: the backlink of `page`, if it is in a reclaim queue still, acquired under the list
    /// lock, as `peek_isolate`'s is: for eviction of an arena's own pages, which finds them by
    /// their chunks rather than in the queues (`Evictor::evict_arena_on_demand`, TASK-79.76).
    pub fn reclaim_backlink(&self, page: VmPagePtr) -> Option<VmoBacklink> {
        let lists = self.list_lock.lock();
        // A page cannot leave the reclaim queues without the lock.
        if !queue_is_reclaim(page.page_queue().load(Ordering::Relaxed)) {
            return None;
        }
        let cow = upgrade_backlink(page);
        // SAFETY: The list lock is held.
        let offset = unsafe { page.object().get_page_offset() };
        drop(lists);
        Some(VmoBacklink { cow, page, offset })
    }

    /// Returns true if the page is in an isolate queue.
    pub fn is_page_reclaimable(page: VmPagePtr) -> bool {
        page.page_queue().load(Ordering::Relaxed) == QUEUE_RECLAIM_ISOLATE
    }

    /// Returns whether the reclaim queues only include pager backed pages.
    pub fn reclaim_is_only_pager_backed(&self) -> bool {
        !self.anonymous_is_reclaimable.load(Ordering::Relaxed)
    }

    /// Looks at the isolate queues and returns backlink information of the first page found. If
    /// the isolate queue is empty then LRU queues up to `lowest_queue` epochs from the most recent
    /// will be processed to attempt to fill the isolate list.
    pub fn peek_isolate(&self, lowest_queue: usize) -> Option<VmoBacklink> {
        // Ignore any requests to evict from the active queues as this is never allowed.
        let lowest_queue = lowest_queue.max(NUM_ACTIVE_QUEUES);
        const MAX_ITERATIONS: u32 = NUM_RECLAIM as u32 * 2;
        let mut loop_iterations = 0;
        loop {
            // Peek the Isolate queue in case anything is ready for us.
            if let Some(result) = self.peek_isolate_list() {
                return Some(result);
            }
            if loop_iterations > MAX_ITERATIONS {
                eprintln!(
                    "[pq]: peek_isolate iterated more than {MAX_ITERATIONS} times. lru:{} mru:{}",
                    self.lru_gen.load(Ordering::Relaxed),
                    self.mru_gen.load(Ordering::Relaxed)
                );
            }
            loop_iterations += 1;
            // Synchronize with any aging that was outstanding at the time of this call, only on
            // the first iteration so that the termination condition is eventually met.
            if loop_iterations == 1 {
                self.synchronize_with_aging();
            }
            // The limit gen is 1 larger than the lowest queue because evicting from queue X is done
            // by attempting to make the lru queue be X+1.
            let lru_limit = self.mru_gen.load(Ordering::Relaxed) - (lowest_queue as u64 - 1);
            // Attempt to process one generation at a time to limit the work done.
            let lru_target = self.lru_gen.load(Ordering::Relaxed) + 1;
            if lru_target > lru_limit {
                return self.peek_isolate_list();
            }
            // Move a few pages for efficiency, leaving the full processing to the lru thread.
            self.process_lru_queue(lru_target, Some(16));
        }
    }

    pub fn get_active_inactive_counts(&self) -> ActiveInactiveCounts {
        let mut counts = ActiveInactiveCounts::default();
        let mru = self.mru_gen_to_queue();
        for queue in 0..NUM_QUEUES as u8 {
            let count = self.count(queue);
            if queue_is_active(queue, mru) {
                counts.active += count;
            }
            if queue_is_inactive(queue, mru) {
                counts.inactive += count;
            }
        }
        counts
    }
}
