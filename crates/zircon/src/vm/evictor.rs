// Copyright 2021 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Frees pages under memory pressure by evicting them.
//!
//! Ported from `zircon/kernel/vm/evictor.cc` and `zircon/kernel/vm/include/vm/evictor.h`.
//!
//! Implements page evictor logic to free pages belonging to a PmmNode under memory pressure.
//! Eviction in this context is direct eviction of pager backed memory.
//!
//! Deviations:
//! - Its thread starts through `crate::sync::spawn`, which with the `shuttle` feature starts
//!   none, as time doesn't pass under Shuttle.
//! - No compression, discardable VMOs or loaned pages, so only pager backed pages are evicted and
//!   `use_compression` is ignored.
//! - The eviction thread is a host thread holding a reference to the evictor, which is shared as
//!   an `Arc`, and counters are plain atomics.
//! - No printing of page queues on a possible livelock.
//! - fxdax: eviction on demand (`EvictionLevel::OnDemand`) takes pages too new for the others'
//!   levels too, for an allocation that finds memory out, with memory never out, aging the queues
//!   for them, a bounded number of times before it reclaims a page (`ON_DEMAND_AGINGS`,
//!   TASK-79.70).
//! - fxdax: an arena's own clean pages are evicted on demand, each alone, the oldest first, found by
//!   its chunks, the oldest first by the page queues' counts of their pages, for the relief of an
//!   arena starved at its own cap (`evict_arena_on_demand`, TASK-79.76); the pages it visits to find
//!   them are counted (`EvictorStats::arena_pages_visited`). The eviction thread evicts them too,
//!   for an arena's allocations without relief (`evict_arena_asynchronous`).

use crate::kernel::deadline::Deadline;
use crate::kernel::event::AutounsignalEvent;
use crate::vm::page::VmPagePtr;
use crate::vm::page_queues::{
    NUM_ACTIVE_QUEUES, NUM_OLDEST_QUEUES, NUM_RECLAIM, PageQueues, RECLAIM_AGES, VmoBacklink,
};
use crate::vm::physmap::{ArenaId, CHUNK_SIZE};
use crate::vm::pmm;
use crate::vm::vm_cow_pages::{
    EvictionAction, ReclaimType, VmCowReclaimFailure, VmCowReclaimResult, VmCowReclaimSuccess,
};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

const PAGE_SIZE: u64 = page::SIZE as u64;
const CHUNK_PAGES: u64 = (CHUNK_SIZE / page::SIZE) as u64;
const KB: u64 = 1024;
const MB: u64 = 1024 * KB;

// vm.reclamation.pages_evicted_pager_backed.total
static PAGER_BACKED_PAGES_EVICTED: AtomicU64 = AtomicU64::new(0);
// vm.reclamation.pages_evicted_pager_backed.oom
static PAGER_BACKED_PAGES_EVICTED_OOM: AtomicU64 = AtomicU64::new(0);
// fxdax: the pages eviction of arenas' own pages has visited to find them
// (`Evictor::evict_arena_on_demand`, TASK-79.76).
static ARENA_PAGES_VISITED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvictionLevel {
    #[default]
    OnlyOldest = 0,
    IncludeNewest = 1,
    /// fxdax: as `IncludeNewest`, and with no page old enough for it, the oldest of those too
    /// new, the active ones, which the queues are aged for: what an allocation that finds no free
    /// page asks for, with memory never out (`OomBehavior::Evict`), as nothing else makes it room,
    /// and pages written as fast as memory takes them are all active (TASK-79.70).
    OnDemand = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Print,
    NoPrint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerReason {
    Oom,
    Other,
}

/// Eviction target state is grouped together behind a lock to allow different threads to safely
/// trigger and perform the eviction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionTarget {
    pub pending: bool,
    /// The desired value to get the PMM's free page count to
    pub free_pages_target: u64,
    /// A minimum amount of pages we want to evict, regardless of how much free memory is
    /// available.
    pub min_pages_to_free: u64,
    pub level: EvictionLevel,
    pub print_counts: bool,
    pub oom_trigger: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictedPageCounts {
    /// evicted from pager-backed VMO page count
    pub pager_backed: u64,
}

impl EvictedPageCounts {
    pub fn non_loaned_total(&self) -> u64 {
        self.pager_backed
    }
}

impl std::ops::AddAssign for EvictedPageCounts {
    fn add_assign(&mut self, counts: Self) {
        self.pager_backed += counts.pager_backed;
    }
}

impl std::ops::Sub for EvictedPageCounts {
    type Output = Self;

    fn sub(self, counts: Self) -> Self {
        Self { pager_backed: self.pager_backed - counts.pager_backed }
    }
}

/// Result returned from eviction methods indicating what they performed and why they returned.
/// Provides by the count of how many different items were evicted, which could be more or less
/// than requested, as well as whether any free memory target was achieved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionResult {
    pub counts: EvictedPageCounts,
    pub free_target_reached: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictorStats {
    pub pager_backed_oom: u64,
    pub pager_backed_other: u64,
    /// fxdax: the pages eviction of arenas' own pages has visited to find them, what a pass of it
    /// costs beyond its evictions (`Evictor::evict_arena_on_demand`, TASK-79.76).
    pub arena_pages_visited: u64,
}

/// Reclaims one page, or returns `None` if there is nothing to reclaim, for tests to fake
/// reclamation.
pub type ReclaimFunction = Box<dyn Fn(EvictionLevel) -> Option<VmCowReclaimResult> + Send + Sync>;
/// Counts the free pages, for tests to fake the PMM.
pub type FreePagesFunction = Box<dyn Fn() -> u64 + Send + Sync>;

/// State protected by the evictor's lock.
struct EvictorLocked {
    /// Target for eviction.
    eviction_target: EvictionTarget,
    /// The eviction thread used to process asynchronous requests.
    /// Created only if eviction is enabled i.e. `eviction_enabled` is set to true.
    eviction_thread: Option<JoinHandle<()>>,
    /// Whether eviction is enabled.
    eviction_enabled: bool,
    /// Whether eviction should attempt compression.
    use_compression: bool,
    /// fxdax: the bytes at least of each arena's own pages to evict on demand, as asked of the
    /// eviction thread (`Evictor::evict_arena_asynchronous`, TASK-79.76), until it takes them.
    arena_targets: BTreeMap<ArenaId, u64>,
}

/// Frees pages belonging to the PMM under memory pressure. This type is thread-safe.
pub struct Evictor {
    lock: Mutex<EvictorLocked>,
    /// Mutex that enforces only one eviction attempt to be active at any time. This prevents us
    /// from overshooting the free memory targets required by various simultaneous eviction
    /// requests. It guards the total amount of evictions performed by `evict_until_targets_met`,
    /// which allows parallel evictions to combine their eviction progress instead of acting
    /// serially.
    eviction_lock: Mutex<EvictedPageCounts>,
    eviction_thread_exiting: AtomicBool,
    /// Used by the eviction thread to wait for eviction requests.
    eviction_signal: AutounsignalEvent,
    /// Optionally specified methods to allow for tests to fake interactions with the pmm.
    test_reclaim_function: Option<ReclaimFunction>,
    test_free_pages_function: Option<FreePagesFunction>,
}

/// The most pages an eviction evicts at a time holding the eviction lock, but for an OOM's, so
/// that parallel attempts at different eviction outcomes do not unnecessarily block each other.
/// fxdax: named, as eviction of an arena's own pages holds it for as many too
/// (`Evictor::evict_arena_on_demand`, TASK-79.76).
const EVICTION_LOCK_PAGES: u64 = 128;

/// fxdax: how many times eviction on demand (`EvictionLevel::OnDemand`) ages the queues, finding
/// no page old enough to evict, before it reclaims one: twice as many times as age the newest pages
/// past the active queues. A page whose reclaim failed is the newest again, and so each is tried
/// twice: the second time, its mappings gone with the first, it fails only if it is in use still,
/// and eviction ends rather than age the queues for it for ever (TASK-79.70).
const ON_DEMAND_AGINGS: usize = 2 * NUM_ACTIVE_QUEUES;

/// Reclaims from the oldest page in the global page queues. fxdax: eviction on demand ages the
/// queues for pages too new for it while `agings`, which each aging takes one of, lets it.
fn reclaim_from_global_page_queues(
    eviction_level: EvictionLevel,
    agings: &mut usize,
) -> Option<(VmCowReclaimResult, VmPagePtr)> {
    // Avoid evicting from the newest queue to prevent thrashing. fxdax: eviction on demand evicts
    // the newest pages too, once it has aged the queues for them (`peek_on_demand`), as nothing
    // else makes room.
    let lowest_evict_queue = if eviction_level >= EvictionLevel::IncludeNewest {
        NUM_ACTIVE_QUEUES
    } else {
        NUM_RECLAIM - NUM_OLDEST_QUEUES
    };
    // If we're going to include newest pages, ignore eviction hints as well, i.e. also consider
    // evicting pages with always_need set if we encounter them in LRU order.
    let hint_action = if eviction_level >= EvictionLevel::IncludeNewest {
        EvictionAction::IgnoreHint
    } else {
        EvictionAction::FollowHint
    };

    let page_queues = pmm::node().page_queues();
    // fxdax: on demand, with the queues aged for pages too new.
    let backlink = if eviction_level == EvictionLevel::OnDemand {
        peek_on_demand(page_queues, lowest_evict_queue, agings)?
    } else {
        page_queues.peek_isolate(lowest_evict_queue)?
    };
    // A valid backlink always has a valid cow
    let cow = backlink.cow.expect("isolated page has a cow");
    let reclaimed = cow.reclaim_page(backlink.page, backlink.offset, hint_action);
    Some((reclaimed, backlink.page))
}

/// fxdax: the oldest page to reclaim on demand (`EvictionLevel::OnDemand`): the oldest old enough
/// to, as with `IncludeNewest`, or, with none, the oldest active one, the queues aged for it rather
/// than when time ages them, as long as `agings`, which each aging takes one of, lets them be
/// (TASK-79.70).
fn peek_on_demand(
    page_queues: &PageQueues,
    lowest_evict_queue: usize,
    agings: &mut usize,
) -> Option<VmoBacklink> {
    loop {
        if let Some(backlink) = page_queues.peek_isolate(lowest_evict_queue) {
            return Some(backlink);
        }
        if *agings == 0 || page_queues.get_active_inactive_counts().active == 0 {
            return None;
        }
        *agings -= 1;
        page_queues.rotate_reclaim_queues();
    }
}

/// fxdax: the `target_pages` oldest pages of `arena` in the reclaim queues, at most, the oldest
/// first by their queues' ages (`PageQueues::visit_reclaim_ages`: those hinted DONT_NEED first of
/// all, as the evictor of the queues takes them, and then the other isolated, older than any
/// generation's), and within an age in the order it visits them: for eviction of the arena's own
/// pages on demand (`Evictor::evict_arena_on_demand`, TASK-79.76).
///
/// It visits the arena's chunks the oldest first, by how old their oldest page may be
/// (`PageQueues::chunk_oldest_age`), and those holding none not at all, and stops once it holds
/// `target_pages`, none younger than a chunk left may hold: it visits about what it may evict, and
/// with nothing to evict, nothing, rather than the whole arena; and holds no more than it returns.
/// It visits more where its chunks' pages were accessed since their queues were last processed, as
/// they are then newer than their chunks say. What is left in proportion to the arena's size is the
/// scan of the chunk table for the arena's chunks, and of their counts. The pages it visits are
/// counted (`EvictorStats::arena_pages_visited`).
fn arena_pages_oldest_first(arena: ArenaId, target_pages: u64) -> Vec<VmPagePtr> {
    let node = pmm::node();
    let page_queues = node.page_queues();
    // The arena's chunks holding pages in the reclaim queues, by how old their oldest may be.
    let mut by_oldest: [Vec<usize>; RECLAIM_AGES] = Default::default();
    for chunk in node.arena_chunks(arena) {
        if let Some(age) = page_queues.chunk_oldest_age(chunk) {
            by_oldest[age].push(chunk);
        }
    }
    let chunks = by_oldest.iter().enumerate().rev();
    let chunks = chunks.flat_map(|(age, chunks)| chunks.iter().map(move |&chunk| (age, chunk)));
    let mut oldest = OldestPages::new(target_pages as usize);
    let mut visited = 0;
    for (chunk_oldest, chunk) in chunks {
        // Neither it nor any chunk after it holds a page older than the youngest held.
        if oldest.holds_as_old_as(chunk_oldest) {
            break;
        }
        visited += CHUNK_PAGES;
        page_queues.visit_reclaim_ages(node.chunk_pages(chunk), |page, age| oldest.push(page, age));
    }
    ARENA_PAGES_VISITED.fetch_add(visited, Ordering::Relaxed);
    oldest.into_oldest_first()
}

/// fxdax: the oldest of the pages it is given, as many as it holds at most, by age
/// (`RECLAIM_AGES`), for `arena_pages_oldest_first` (TASK-79.76).
struct OldestPages {
    by_age: [Vec<VmPagePtr>; RECLAIM_AGES],
    held: usize,
    capacity: usize,
}

impl OldestPages {
    fn new(capacity: usize) -> Self {
        Self { by_age: Default::default(), held: 0, capacity }
    }

    /// Holds `page`, of age `age`, if it has room, or else in the place of the youngest held, the
    /// last given of them, if `page` is older.
    fn push(&mut self, page: VmPagePtr, age: usize) {
        if self.held == self.capacity {
            let Some(youngest) = self.youngest().filter(|&youngest| youngest < age) else { return };
            self.by_age[youngest].pop();
        } else {
            self.held += 1;
        }
        self.by_age[age].push(page);
    }

    /// The age of the youngest held, if any.
    fn youngest(&self) -> Option<usize> {
        self.by_age.iter().position(|pages| !pages.is_empty())
    }

    /// Whether it holds as many as it may, none younger than `age`: whether a page of `age` or
    /// younger would be held.
    fn holds_as_old_as(&self, age: usize) -> bool {
        self.held == self.capacity && self.youngest().is_none_or(|youngest| youngest >= age)
    }

    /// The pages held, the oldest first.
    fn into_oldest_first(self) -> Vec<VmPagePtr> {
        self.by_age.into_iter().rev().flatten().collect()
    }
}

/// fxdax: evicts `page` alone, on demand, if it is a clean page of an object of `arena`'s in the
/// reclaim queues still, and returns how many pages it evicted, for `Evictor::evict_arena_on_demand`
/// (TASK-79.76). Alone, rather than with its batch of 16 as `VmCowPages::reclaim_page` evicts it,
/// whose pages on demand would go however new.
fn evict_arena_page(arena: ArenaId, page: VmPagePtr) -> u64 {
    // It may have been evicted since, or freed; or its chunk gone back, and come to another arena,
    // whose object's it is now.
    let Some(VmoBacklink { cow: Some(cow), offset, .. }) =
        pmm::node().page_queues().reclaim_backlink(page)
    else {
        return 0;
    };
    if cow.arena() != arena || !cow.can_evict() {
        return 0;
    }
    let evicted = cow.reclaim_range_for_eviction(offset, PAGE_SIZE, EvictionAction::Demand);
    evicted.map_or(0, |success| success.num_pages)
}

#[derive(Default)]
struct FailureReasons {
    compress_failed: u64,
    compress_accessed: u64,
    evict_accessed: u64,
    incorrect_page: u64,
    other: u64,
}

struct ReclaimFailureStats {
    consecutive_reclaim_failures: u64,
    failures_to_compare: u64,
    prev_page_evictions: u64,
    prev_evicted_page: Option<VmPagePtr>,
    prev_eviction_result: VmCowReclaimResult,
    printed_same_page_log: bool,
    failure_reasons: FailureReasons,
}

impl ReclaimFailureStats {
    const COMPARISON_BASE: u64 = 1000;

    fn new() -> Self {
        Self {
            consecutive_reclaim_failures: 0,
            failures_to_compare: Self::COMPARISON_BASE,
            prev_page_evictions: 0,
            prev_evicted_page: None,
            // An arbitrary value; this will be overwritten anyway by the actual result of eviction
            // when one happens.
            prev_eviction_result: Ok(VmCowReclaimSuccess {
                r#type: ReclaimType::Evict,
                num_pages: 0,
            }),
            printed_same_page_log: false,
            failure_reasons: FailureReasons::default(),
        }
    }

    fn update(&mut self, reclaim_failed: bool) {
        if reclaim_failed {
            self.consecutive_reclaim_failures += 1;
        } else {
            self.consecutive_reclaim_failures = 0;
            self.failures_to_compare = Self::COMPARISON_BASE;
            self.failure_reasons = FailureReasons::default();
        }
    }

    fn should_print_livelock(&mut self) -> bool {
        if self.consecutive_reclaim_failures == self.failures_to_compare {
            // Wait for longer before logging the next time to avoid spamming in an actual
            // livelock. Multiply checking for overflow.
            if let Some(next) = self.failures_to_compare.checked_mul(10) {
                self.failures_to_compare = next;
            }
            return true;
        }
        false
    }

    fn to_result_string(result: VmCowReclaimResult) -> &'static str {
        match result {
            Err(VmCowReclaimFailure::CompressFailed) => "fail:compress",
            Err(VmCowReclaimFailure::CompressAccessed) => "fail:access_c",
            Err(VmCowReclaimFailure::EvictAccessed) => "fail:access_e",
            Err(VmCowReclaimFailure::IncorrectPage) => "fail:incorrect",
            Err(VmCowReclaimFailure::Other) => "fail:other",
            Ok(VmCowReclaimSuccess { r#type: ReclaimType::Compress, .. }) => "ok:compress",
            Ok(VmCowReclaimSuccess { r#type: ReclaimType::Discard, .. }) => "ok:discard",
            Ok(VmCowReclaimSuccess { r#type: ReclaimType::Evict, .. }) => "ok:evict",
        }
    }

    fn check_for_same_page(&mut self, reclaimed: (VmCowReclaimResult, VmPagePtr)) {
        let evicted_page = reclaimed.1;
        let prev_evicted_page = self.prev_evicted_page.replace(evicted_page);
        let prev_eviction_result = std::mem::replace(&mut self.prev_eviction_result, reclaimed.0);

        if prev_evicted_page != Some(evicted_page) {
            return;
        }

        // Evicting the same page twice in a row indicates a potential bug in reclamation, unless
        // we're in the IncorrectPage failure case, since that indicates a race in page ownership,
        // and we can't expect the VmCowPages to have moved a page it does not own out of the way.
        // Every other failure case should have moved the page out of the way (by calling
        // MarkAccessed), so we should not see the same page in the next eviction attempt.
        //
        // It is also possible to see the same page in rare hard-to-check-for cases after having
        // been reclaimed and reused. See the discussion in https://fxbug.dev/434361683.
        if prev_eviction_result == Err(VmCowReclaimFailure::IncorrectPage) {
            return;
        }

        self.prev_page_evictions += 1;
        // Print only once per eviction attempt to prevent log spam.
        if !self.printed_same_page_log {
            self.printed_same_page_log = true;
            eprintln!(
                "Evictor reclaiming the same page again {:?} [prev {} cur {}]",
                evicted_page,
                Self::to_result_string(prev_eviction_result),
                Self::to_result_string(reclaimed.0)
            );
        }
    }
}

fn diagnose_reclamation_failure(
    reclaimed: (VmCowReclaimResult, Option<VmPagePtr>),
    failure_stats: &mut ReclaimFailureStats,
) {
    let mut reclaim_failed = false;
    if let Err(failure) = reclaimed.0 {
        reclaim_failed = true;
        let reasons = &mut failure_stats.failure_reasons;
        match failure {
            VmCowReclaimFailure::CompressFailed => reasons.compress_failed += 1,
            VmCowReclaimFailure::CompressAccessed => reasons.compress_accessed += 1,
            VmCowReclaimFailure::EvictAccessed => reasons.evict_accessed += 1,
            VmCowReclaimFailure::IncorrectPage => reasons.incorrect_page += 1,
            VmCowReclaimFailure::Other => reasons.other += 1,
        }
    }

    // The test reclaim path might not even evict an actual page, so there's nothing to check.
    if let Some(page) = reclaimed.1 {
        failure_stats.check_for_same_page((reclaimed.0, page));
    }

    // If we've looped many times without being able to reclaim anything, make some noise.
    failure_stats.update(reclaim_failed);
    if failure_stats.should_print_livelock() {
        let reasons = &failure_stats.failure_reasons;
        eprintln!(
            "Evictor failed {} reclaims in a row ({} for prev page), possible livelock\n\
             compress_failed {} compress_accessed {} evict_accessed {} incorrect_page {} other {}",
            failure_stats.consecutive_reclaim_failures,
            failure_stats.prev_page_evictions,
            reasons.compress_failed,
            reasons.compress_accessed,
            reasons.evict_accessed,
            reasons.incorrect_page,
            reasons.other
        );
    }
}

impl Default for Evictor {
    fn default() -> Self {
        Self::new()
    }
}

impl Evictor {
    pub fn new() -> Self {
        Self::with_functions(None, None)
    }

    /// Creates an evictor with custom methods to fake the reclamation, for tests.
    pub fn with_functions(
        reclaim_function: Option<ReclaimFunction>,
        free_pages_function: Option<FreePagesFunction>,
    ) -> Self {
        Self {
            lock: Mutex::new(EvictorLocked {
                eviction_target: EvictionTarget::default(),
                eviction_thread: None,
                eviction_enabled: false,
                use_compression: false,
                arena_targets: BTreeMap::new(),
            }),
            eviction_lock: Mutex::new(EvictedPageCounts::default()),
            eviction_thread_exiting: AtomicBool::new(false),
            eviction_signal: AutounsignalEvent::new(false),
            test_reclaim_function: reclaim_function,
            test_free_pages_function: free_pages_function,
        }
    }

    /// Return global eviction stats from all instantiations of the Evictor.
    pub fn get_global_stats() -> EvictorStats {
        let pager_backed_oom = PAGER_BACKED_PAGES_EVICTED_OOM.load(Ordering::Relaxed);
        EvictorStats {
            pager_backed_oom,
            pager_backed_other: PAGER_BACKED_PAGES_EVICTED.load(Ordering::Relaxed)
                - pager_backed_oom,
            arena_pages_visited: ARENA_PAGES_VISITED.load(Ordering::Relaxed),
        }
    }

    /// Whether any eviction can occur.
    pub fn is_eviction_enabled(&self) -> bool {
        self.lock.lock().eviction_enabled
    }

    /// Called from the scanner to enable eviction if required. Creates an eviction thread to
    /// process asynchronous eviction requests.
    pub fn enable_eviction(self: &Arc<Self>, use_compression: bool) {
        let mut locked = self.lock.lock();
        // It's an error to call this whilst the eviction thread is still exiting.
        assert!(!self.eviction_thread_exiting.load(Ordering::Acquire));
        locked.eviction_enabled = true;
        locked.use_compression = use_compression;

        if locked.eviction_thread.is_some() {
            return;
        }

        // Set up the eviction thread to process asynchronous eviction requests.
        let evictor = self.clone();
        let thread = crate::sync::spawn("eviction-thread", move || evictor.eviction_thread_loop());
        locked.eviction_thread = thread;
    }

    /// Called from the scanner to disable all eviction if needed, will shut down any in existing
    /// eviction thread. It is a responsibility of the scanner to not have multiple concurrent
    /// calls to this and `enable_eviction`.
    pub fn disable_eviction(&self) {
        let eviction_thread = {
            // Grab the lock and update any state. We cannot actually wait for the eviction thread
            // to complete whilst the lock is held, however.
            let mut locked = self.lock.lock();
            let Some(eviction_thread) = locked.eviction_thread.take() else {
                return;
            };
            // It's an error to call this in parallel with another disable_eviction call.
            assert!(!self.eviction_thread_exiting.load(Ordering::Acquire));
            self.eviction_thread_exiting.store(true, Ordering::Release);
            self.eviction_signal.signal();
            eviction_thread
        };
        // Now with the lock dropped wait for the thread to complete.
        let joined = eviction_thread.join();
        debug_assert!(joined.is_ok());
        // Now update the state to indicate that eviction is disabled.
        let mut locked = self.lock.lock();
        locked.eviction_enabled = false;
        self.eviction_thread_exiting.store(false, Ordering::Release);
    }

    pub fn debug_get_eviction_target(&self) -> EvictionTarget {
        self.lock.lock().eviction_target
    }

    /// Combine the specified `target` with the pre-existing eviction target.
    pub fn combine_eviction_target(&self, target: EvictionTarget) {
        let mut locked = self.lock.lock();
        let eviction_target = &mut locked.eviction_target;
        eviction_target.pending = eviction_target.pending || target.pending;
        eviction_target.level = eviction_target.level.max(target.level);
        eviction_target.min_pages_to_free = eviction_target
            .min_pages_to_free
            .checked_add(target.min_pages_to_free)
            .expect("min pages to free overflow");
        eviction_target.free_pages_target =
            eviction_target.free_pages_target.max(target.free_pages_target);
        eviction_target.print_counts = eviction_target.print_counts || target.print_counts;
    }

    /// Evict from a user specified external `target` which is only used for this eviction attempt
    /// and does not interfere with the evictor's own target.
    pub fn evict_from_external_target(&self, target: EvictionTarget) -> EvictionResult {
        self.evict_from_target_internal(target)
    }

    /// Perform eviction based on the current values of the eviction target. The expectation is
    /// that the user will have set the target before calling this function with
    /// `combine_eviction_target`. This may acquire arbitrary vmo and aspace locks.
    pub fn evict_from_preloaded_target(&self) -> EvictedPageCounts {
        // Create a local copy of the eviction target to operate against.
        let target = self.lock.lock().eviction_target;
        let result = self.evict_from_target_internal(target);
        let mut locked = self.lock.lock();
        let total = result.counts.non_loaned_total();
        // Clear the eviction target but retain any min pages that we might still need to free in
        // a subsequent eviction attempt.
        locked.eviction_target = EvictionTarget {
            min_pages_to_free: target.min_pages_to_free.saturating_sub(total),
            ..EvictionTarget::default()
        };
        result.counts
    }

    fn evict_from_target_internal(&self, target: EvictionTarget) -> EvictionResult {
        if !target.pending {
            return EvictionResult::default();
        }

        let free_pages_before = self.count_free_pages();
        let result = self.evict_until_targets_met(
            target.min_pages_to_free,
            target.free_pages_target,
            target.level,
            target.oom_trigger,
        );
        let free_pages_after = self.count_free_pages();

        if target.print_counts {
            // Helper to print the counts as values in KB or MB.
            let format = |count: u64| {
                if count < MB / PAGE_SIZE {
                    format!("{}K", count * PAGE_SIZE / KB)
                } else {
                    format!("{}M", count * PAGE_SIZE / MB)
                }
            };
            if result.counts.pager_backed > 0 {
                eprintln!(
                    "[EVICT]: pager:{} free:{}M->{}M",
                    format(result.counts.pager_backed),
                    free_pages_before * PAGE_SIZE / MB,
                    free_pages_after * PAGE_SIZE / MB
                );
            }
        }

        if target.oom_trigger {
            PAGER_BACKED_PAGES_EVICTED_OOM.fetch_add(result.counts.pager_backed, Ordering::Relaxed);
        }

        result
    }

    /// Performs a synchronous request to evict until free memory equals `free_mem_target` (in
    /// bytes) and at least `min_mem_to_free` (in bytes) has been reclaimed. The return value is an
    /// EvictionResult detailing exactly what was evicted and whether the free_mem_target was
    /// achieved. The `eviction_level` is a rough control that maps to how old a page needs to be
    /// for being considered for eviction. This may acquire arbitrary vmo and aspace locks.
    pub fn evict_synchronous(
        &self,
        min_mem_to_free: u64,
        free_mem_target: u64,
        eviction_level: EvictionLevel,
        output: Output,
        reason: TriggerReason,
    ) -> EvictionResult {
        if !self.is_eviction_enabled() {
            return EvictionResult::default();
        }
        let target = EvictionTarget {
            pending: true,
            free_pages_target: free_mem_target / PAGE_SIZE,
            min_pages_to_free: min_mem_to_free / PAGE_SIZE,
            level: eviction_level,
            print_counts: output == Output::Print,
            oom_trigger: reason == TriggerReason::Oom,
        };

        self.evict_from_external_target(target)
    }

    /// Reclaim memory until free memory equals the `free_mem_target` (in bytes) and at least
    /// `min_mem_to_free` (in bytes) has been reclaimed. Reclamation will happen asynchronously on
    /// the eviction thread and this function returns immediately. Once the target is reached, or
    /// there is no more memory that can be reclaimed, this process will stop and the free memory
    /// target will be cleared. The `eviction_level` is a rough control on how hard to try and
    /// evict. Multiple calls to evict_asynchronous will cause all the targets to get merged by
    /// adding together `min_mem_to_free`, taking the max of `free_mem_target` and the highest or
    /// most aggressive of any `eviction_level`.
    pub fn evict_asynchronous(
        &self,
        min_mem_to_free: u64,
        free_mem_target: u64,
        eviction_level: EvictionLevel,
        output: Output,
    ) {
        if !self.is_eviction_enabled() {
            return;
        }
        self.combine_eviction_target(EvictionTarget {
            pending: true,
            free_pages_target: free_mem_target / PAGE_SIZE,
            min_pages_to_free: min_mem_to_free / PAGE_SIZE,
            level: eviction_level,
            print_counts: output == Output::Print,
            oom_trigger: false,
        });
        // Unblock the eviction thread.
        self.eviction_signal.signal();
    }

    /// fxdax: evicts at least `min_mem_to_free` bytes of `arena`'s own clean pages on demand, as
    /// `evict_arena_on_demand` does, on the eviction thread, and returns at once: for an allocation
    /// of an arena starved at its own cap without relief, which only pages of its own make room
    /// for (TASK-79.76). Calls for an arena before the thread takes them merge, taking the most of
    /// their `min_mem_to_free`, rather than add up as `evict_asynchronous`'s do: the arena's
    /// allocations each ask as they find it starved, and as they try again, and what one asks for
    /// makes room for many; those it leaves short ask again.
    pub fn evict_arena_asynchronous(&self, arena: ArenaId, min_mem_to_free: u64) {
        let mut locked = self.lock.lock();
        if !locked.eviction_enabled {
            return;
        }
        let target = locked.arena_targets.entry(arena).or_default();
        *target = (*target).max(min_mem_to_free);
        drop(locked);
        self.eviction_signal.signal();
    }

    /// Evict until `min_pages_to_evict` have been evicted and there are at least
    /// `free_pages_target` free pages on the system. Note that the eviction operation here is
    /// one-shot, i.e. as soon as the targets are met, eviction will stop and the function will
    /// return. Returns the number of pages evicted during the execution of this method. These
    /// counts include all parallel invocations of this method and could be higher than requested
    /// if there was another eviction happening. This may acquire arbitrary vmo and aspace locks.
    fn evict_until_targets_met(
        &self,
        min_pages_to_evict: u64,
        free_pages_target: u64,
        level: EvictionLevel,
        oom_trigger: bool,
    ) -> EvictionResult {
        if !self.is_eviction_enabled() {
            return EvictionResult::default();
        }

        // Take a snapshot of the eviction counts at the beginning of our attempts.
        let starting_counts = *self.eviction_lock.lock();

        let mut free_target_reached = false;
        loop {
            // Lock the eviction attempts so that we don't overshoot the free pages target.
            let mut total_evicted = self.eviction_lock.lock();

            let pages_freed_so_far =
                total_evicted.non_loaned_total() - starting_counts.non_loaned_total();

            let free_pages = self.count_free_pages();
            let mut pages_to_free = 0;
            // Need to evict at least min_pages_to_evict, and then potentially up to the
            // free_pages_target.
            if pages_freed_so_far < min_pages_to_evict {
                pages_to_free = min_pages_to_evict - pages_freed_so_far;
            }
            if free_pages < free_pages_target {
                pages_to_free = (free_pages_target - free_pages).max(pages_to_free);
            } else {
                free_target_reached = true;
            }
            if pages_to_free == 0 {
                // The targets have been met. No more eviction is required right now.
                break;
            }

            // Cap the number of pages to free so that we do not monopolize the eviction lock,
            // ensuring that parallel attempts at achieving different eviction outcomes do not
            // unnecessarily block each other.
            //
            // For OOM evictions we do not cap the number of pages so that we can evict as fast as
            // possible to resolve the OOM condition quickly.
            if !oom_trigger {
                pages_to_free = pages_to_free.min(EVICTION_LOCK_PAGES);
            }

            let pages_freed = self.evict_page_queues(pages_to_free, level);
            // Should we fail to free any pages then we give up and consider the eviction request
            // complete.
            if pages_freed.non_loaned_total() == 0 {
                break;
            }
            *total_evicted += pages_freed;
        }

        // Return the difference of the current eviction counts compared to when we started.
        EvictionResult { counts: *self.eviction_lock.lock() - starting_counts, free_target_reached }
    }

    /// Evict the requested number of `target_pages` from vmos by querying the page queues. The
    /// returned struct has the number of pages evicted. The `eviction_level` is a rough control
    /// that maps to how old a page needs to be for being considered for eviction. This may acquire
    /// arbitrary vmo and aspace locks.
    fn evict_page_queues(
        &self,
        target_pages: u64,
        eviction_level: EvictionLevel,
    ) -> EvictedPageCounts {
        let mut counts = EvictedPageCounts::default();

        if !self.is_eviction_enabled() {
            return counts;
        }

        let mut failure_stats = ReclaimFailureStats::new();
        // fxdax: the agings eviction on demand may make before it next reclaims a page, so that it
        // ends, as the others do, once every page it may evict has failed (TASK-79.70).
        let mut agings = ON_DEMAND_AGINGS;
        // Evict until we've counted enough pages to hit the target_pages.
        while counts.non_loaned_total() < target_pages {
            // Use the helper to perform a single 'step' of eviction.
            // An empty return from the helper indicates that there are no more eviction
            // candidates, so regardless of our desired target we must give up.
            let Some(reclaimed) = self.evict_page_queues_helper(eviction_level, &mut agings) else {
                break;
            };

            // Check for any reclamation failures and print diagnostic logs if needed.
            diagnose_reclamation_failure(reclaimed, &mut failure_stats);

            if let Ok(success) = reclaimed.0 {
                agings = ON_DEMAND_AGINGS;
                match success.r#type {
                    ReclaimType::Evict => counts.pager_backed += success.num_pages,
                    ReclaimType::Discard | ReclaimType::Compress => {
                        unreachable!("no discardable VMOs or compression")
                    }
                }
            }
        }

        PAGER_BACKED_PAGES_EVICTED.fetch_add(counts.pager_backed, Ordering::Relaxed);
        counts
    }

    /// fxdax: evicts at least `min_mem_to_free` bytes of the clean pages of `arena`'s objects, the
    /// oldest first, but of any age, on demand (`EvictionAction::Demand`), and returns how many it
    /// evicted: for relief of an arena starved at its own cap, which only pages of its own make
    /// room for, and for which no other arena's page, however old, is evicted (TASK-79.76). It
    /// finds them by the arena's chunks (`arena_pages_oldest_first`), rather than in the queues,
    /// which hold every arena's, and ages none; and evicts each alone (`evict_arena_page`). Pages it
    /// fails to evict, dirtied or freed since it found them, a walk of the chunks again makes up
    /// for, until one evicts none.
    pub fn evict_arena_on_demand(&self, arena: ArenaId, min_mem_to_free: u64) -> EvictedPageCounts {
        let mut counts = EvictedPageCounts::default();
        if !self.is_eviction_enabled() {
            return counts;
        }
        let target_pages = min_mem_to_free.div_ceil(PAGE_SIZE);
        while counts.non_loaned_total() < target_pages {
            let walked = counts;
            let oldest = arena_pages_oldest_first(arena, target_pages - counts.non_loaned_total());
            // One eviction at a time, as `evict_until_targets_met`'s, and as there, the eviction
            // lock held for at most `EVICTION_LOCK_PAGES` pages at a time.
            for pages in oldest.chunks(EVICTION_LOCK_PAGES as usize) {
                let mut total_evicted = self.eviction_lock.lock();
                let evicted = pages.iter().map(|&page| evict_arena_page(arena, page)).sum();
                let evicted = EvictedPageCounts { pager_backed: evicted };
                *total_evicted += evicted;
                counts += evicted;
            }
            if counts == walked {
                break;
            }
        }
        PAGER_BACKED_PAGES_EVICTED.fetch_add(counts.pager_backed, Ordering::Relaxed);
        counts
    }

    /// The main loop for the eviction thread.
    fn eviction_thread_loop(&self) {
        while !self.eviction_thread_exiting.load(Ordering::Acquire) {
            let status = self.eviction_signal.wait(&Deadline::infinite());
            debug_assert!(status.is_ok());

            if self.eviction_thread_exiting.load(Ordering::Acquire) {
                break;
            }

            // Process an eviction target if there is one. This is a no-op and no pages are evicted
            // if no target is pending.
            self.evict_from_preloaded_target();
            // fxdax: and the arenas' own asked for (TASK-79.76).
            let arena_targets = std::mem::take(&mut self.lock.lock().arena_targets);
            for (arena, min_mem_to_free) in arena_targets {
                self.evict_arena_on_demand(arena, min_mem_to_free);
            }
        }
    }

    /// Returns the count of the free pages for use in performing target calculations. This could
    /// be from the PMM or a test fake.
    fn count_free_pages(&self) -> u64 {
        if let Some(test_free_pages_function) = &self.test_free_pages_function {
            return test_free_pages_function();
        }
        pmm::count_free_pages()
    }

    /// Helper method that performs a single 'step' of reclamation via the page queues. A return
    /// value of some (even if the counts themselves are all zero) indicates that there is more to
    /// reclamation that can be attempted, where as `None` indicates there is nothing left to try
    /// and reclaim.
    /// This method will either reclaim from the global page queues, or receive fake data from a
    /// test instance.
    /// The page that reclamation was attempted on is also returned for debug purposes. This is
    /// only populated for a real reclamation, not a fake one from a test instance.
    /// fxdax: eviction on demand ages the queues while `agings` lets it (`peek_on_demand`).
    fn evict_page_queues_helper(
        &self,
        eviction_level: EvictionLevel,
        agings: &mut usize,
    ) -> Option<(VmCowReclaimResult, Option<VmPagePtr>)> {
        if let Some(test_reclaim_function) = &self.test_reclaim_function {
            return test_reclaim_function(eviction_level).map(|result| (result, None));
        }
        reclaim_from_global_page_queues(eviction_level, agings)
            .map(|(result, page)| (result, Some(page)))
    }
}
