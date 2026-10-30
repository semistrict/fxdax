// Copyright 2026 The Fuchsia Authors.
// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The physical memory manager.
//!
//! Lifted from fxdax's port of the Zircon kernel's PMM (`zircon/kernel/vm/pmm_node.rs`), its arenas
//! alone: what is fxdax's own of it, without the kernel's pages' structures, page queues, evictor,
//! memory watchdog and its gate, or loaned pages.
//!
//! The PMM owns every page of physical memory (see `physmap`) and hands them out one at a time,
//! each a frame (`frames`) its holder frees by dropping.
//!
//! - Physical memory is in arenas, each its own shared memory object (see `physmap`, TASK-43),
//!   which take it a chunk at a time: the free list is the arenas' (`arenas`), each arena's the
//!   free pages of its chunks. Free memory counts every free page, whichever arena's. An arena
//!   with no free page that can take no chunk, even with other arenas' spare chunks gone back for
//!   one, is starved (`Arenas::starved`): past its own cap, or, past the arenas' together
//!   (TASK-50.3) or out of chunks, with too few of their spares. Its allocations are relieved
//!   (`relief`), and, as they may, wait for it; without relief they fail, as nothing makes room.
//!   One of an arena removed fails, whether or not it may wait: nothing makes the arena room
//!   (TASK-79.36). So does one the host maps no chunk for, as the process makes as many mappings
//!   as it may (`physmap`, TASK-79.37), reported as the node's failure.
//! - A chunk given back goes back to the host as the node's lock is dropped, outside it
//!   (`NodeGuard`, TASK-92.35), as its system calls would hold every allocation up for as long
//!   under it; an allocation that it would make room for waits for it.
//! - A page a hypervisor's second stage mapped since it was allocated, freed, is quarantined until
//!   every second stage has made the changes taking it away (`quarantine`, TASK-79.9).

use crate::frames::Frames;
use crate::physmap::{ArenaId, CHUNK_SIZE, PhysMemory};
use crate::sync::Pulse;
use crate::{NoMemory, Wait, page_bytes};
use arenas::{Arenas, Refused};
use parking_lot::{Mutex, MutexGuard};
use std::ops::{Deref, DerefMut};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

mod arenas;
pub(crate) mod quarantine;
pub(crate) mod relief;

/// The pages of physical memory and their frames.
pub(crate) struct PmmPages {
    pub(crate) memory: PhysMemory,
    pub(crate) frames: Frames,
}

impl PmmPages {
    pub(crate) fn new(size: usize) -> Self {
        assert!(CHUNK_SIZE.is_multiple_of(page_bytes()), "a chunk is not whole pages");
        let memory = PhysMemory::new(size);
        let frames = Frames::new(size / page_bytes());
        Self { memory, frames }
    }

    /// The physical address of `frame`'s page.
    pub(crate) fn paddr(frame: u32) -> usize {
        frame as usize * page_bytes()
    }

    /// The chunk `frame`'s page is in.
    pub(crate) fn chunk_of(&self, frame: u32) -> usize {
        self.memory.chunk_of(Self::paddr(frame))
    }
}

/// Why an allocation gave no page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failed {
    /// Nothing makes it room: it fails.
    NoMemory,
    /// Its arena is starved and relief asked for, which it waits for, and tries again.
    ShouldWait,
}

/// fxdax: the node's lock, held (TASK-92.35). As it is dropped, the host takes back the chunks the
/// arenas gave back under it, each without the lock, which it takes again to record what the host
/// did ([`Arenas::returned`]), and to tell allocations waiting for room: reserving a chunk's
/// addresses again and freeing its memory are system calls, which every allocation, of any arena's,
/// would otherwise wait for, and which none waits for now but one the chunk makes room for
/// (`Refused::GoingBack`).
struct NodeGuard<'a> {
    node: &'a PmmNode,
    locked: MutexGuard<'a, Arenas>,
}

impl Deref for NodeGuard<'_> {
    type Target = Arenas;

    fn deref(&self) -> &Arenas {
        &self.locked
    }
}

impl DerefMut for NodeGuard<'_> {
    fn deref_mut(&mut self) -> &mut Arenas {
        &mut self.locked
    }
}

impl Drop for NodeGuard<'_> {
    fn drop(&mut self) {
        let pages = &self.node.pages;
        for returning in self.locked.take_returning() {
            let returned = MutexGuard::unlocked(&mut self.locked, || returning.release(pages));
            self.locked.returned(pages, returned);
            self.node.room.pulse();
        }
    }
}

/// A node of physical memory.
pub(crate) struct PmmNode {
    /// The arenas: the free lists, and the chunks no arena has.
    lock: Mutex<Arenas>,
    // This is both an atomic and guarded by lock as we would like modifications to require the
    // lock, as logic in the system relies on the free_count not changing whilst the lock is
    // held, but also be an atomic so it can be correctly read without the lock.
    free_count: AtomicU64,
    /// Indicates whether an allocation has ever failed as memory is out.
    alloc_failed_no_mem: AtomicBool,
    /// fxdax: pulsed as an allocation of a starved arena may find room, or must fail now: a page or
    /// a chunk freed, a cap changed, relief's pass for an arena done, relief gone (TASK-50.3).
    /// Every waiter wakes, as the waiters of one arena's may not be those of the next's.
    room: Pulse,
    pages: PmmPages,
}

impl PmmNode {
    fn new(size: usize) -> Self {
        let pages = PmmPages::new(size);
        let free_count = AtomicU64::new(pages.frames.len() as u64);
        let chunks = pages.memory.chunk_count();
        Self {
            lock: Mutex::new(Arenas::new(chunks)),
            free_count,
            alloc_failed_no_mem: AtomicBool::new(false),
            room: Pulse::new(),
            pages,
        }
    }

    pub(crate) fn phys_memory(&self) -> &PhysMemory {
        &self.pages.memory
    }

    pub(crate) fn frames(&self) -> &Frames {
        &self.pages.frames
    }

    /// The chunk `frame`'s page is in.
    #[cfg(all(test, not(feature = "shuttle")))]
    pub(crate) fn chunk_of(&self, frame: u32) -> usize {
        self.pages.chunk_of(frame)
    }

    /// Acquires the node's lock (`NodeGuard`).
    fn locked(&self) -> NodeGuard<'_> {
        NodeGuard { node: self, locked: self.lock.lock() }
    }

    /// fxdax: drops `locked`, under which an allocation found chunks going back that would make it
    /// room (`Refused::GoingBack`), and waits for one to go back, or for other room, before it
    /// tries again (TASK-92.35): as it waited for the lock while the host took them back under it.
    fn wait_for_chunks_going_back(&self, locked: NodeGuard<'_>) {
        let room = self.room.count();
        drop(locked);
        self.room.wait_past(room);
    }

    /// Allocates a page of `arena`, its frame: one that may `wait` waits, while its arena is
    /// starved, for relief to make it room, and tries again; and fails once nothing will.
    pub(crate) fn alloc(&self, arena: ArenaId, wait: Wait) -> Result<u32, NoMemory> {
        Ok(self.alloc_run(arena, 1, wait)?[0])
    }

    /// fxdax: allocates `count` pages of `arena`, their frames, as [`PmmNode::alloc`] does one:
    /// those after each consecutive where they can be, each run taken under one hold of the lock,
    /// so that allocations at once take runs of their own rather than pages turn about
    /// (TASK-108.1). One that fails frees what it took.
    pub(crate) fn alloc_run(
        &self,
        arena: ArenaId,
        count: usize,
        wait: Wait,
    ) -> Result<Vec<u32>, NoMemory> {
        let mut frames = Vec::with_capacity(count);
        while frames.len() < count {
            // What it waits for comes after it tries.
            let room = self.room.count();
            match self.try_alloc_run(arena, count - frames.len(), wait) {
                Ok(run) => frames.extend(run),
                Err(Failed::NoMemory) => {
                    for frame in frames {
                        self.free(frame);
                    }
                    return Err(NoMemory);
                }
                Err(Failed::ShouldWait) => {
                    if self.starves(arena) {
                        self.room.wait_past(room);
                    }
                }
            }
        }
        Ok(frames)
    }

    /// Allocates a run of pages of `arena`, once, `most` or fewer, of its smallest free block that
    /// holds them (`FreeBlocks::take`), and as many of the pages after them as are free, up to
    /// `most` in all, under one hold of the lock: told to wait, as its arena is starved and relief
    /// asked for, the caller waits for room, and tries again.
    fn try_alloc_run(&self, arena: ArenaId, most: usize, wait: Wait) -> Result<Vec<u32>, Failed> {
        loop {
            let mut locked = self.locked();
            let (first, count) = match locked.take(&self.pages, arena, most) {
                Ok(taken) => taken,
                Err(Refused::Starved(starved)) => {
                    return Err(self.starved_status(locked, arena, starved, wait));
                }
                Err(Refused::Unmapped) => return Err(self.unmapped_status()),
                Err(Refused::GoingBack) => {
                    self.wait_for_chunks_going_back(locked);
                    continue;
                }
            };
            let mut run: Vec<u32> = (first..first + count as u32).collect();
            for &frame in &run {
                self.pages.frames.allocate(frame);
                self.decrement_free_count();
            }
            while run.len() < most {
                let Some(next) = locked.take_after(&self.pages, *run.last().unwrap()) else {
                    break;
                };
                self.pages.frames.allocate(next);
                self.decrement_free_count();
                run.push(next);
            }
            return Ok(run);
        }
    }

    /// fxdax: allocates the page physically after `frame`'s, if it is free, of the same chunk and
    /// arena (`Arenas::take_after`, TASK-108.1), its frame: so that the pages an allocation takes
    /// together are consecutive where they can be.
    pub(crate) fn alloc_after(&self, frame: u32) -> Option<u32> {
        let mut locked = self.locked();
        let next = locked.take_after(&self.pages, frame)?;
        self.pages.frames.allocate(next);
        self.decrement_free_count();
        drop(locked);
        Some(next)
    }

    /// fxdax: whether `arena` is what an allocation of it told to wait waits for: it is starved
    /// (`Arenas::starved`), with no free page of its own and no chunk to take, even with other
    /// arenas' spares gone back for one.
    fn starves(&self, arena: ArenaId) -> bool {
        self.locked().starved(arena).is_some()
    }

    /// fxdax: caps the memory `arena` may take, in bytes, rounded down to chunks (2 MiB), or lifts
    /// its cap. Lowered under what it has, it takes no more until it has less (TASK-50.3).
    pub(crate) fn set_arena_cap(&self, arena: ArenaId, bytes: Option<u64>) {
        self.locked().set_cap(arena, bytes.map(chunks_in));
        // Waiters may find room now.
        self.room.pulse();
    }

    /// fxdax: caps the memory the arenas may take together, in bytes, rounded down to chunks, or
    /// lifts the cap.
    pub(crate) fn set_total_cap(&self, bytes: Option<u64>) {
        self.locked().set_total_cap(bytes.map(chunks_in));
        self.room.pulse();
    }

    /// fxdax: the cap of the memory the arenas may take together, in bytes, if any.
    pub(crate) fn total_cap(&self) -> Option<u64> {
        self.locked().total_cap().map(chunk_bytes)
    }

    /// fxdax: the memory `arena` has, in bytes: its chunks', free pages and spares included.
    pub(crate) fn arena_bytes(&self, arena: ArenaId) -> u64 {
        chunk_bytes(self.locked().chunks(arena))
    }

    /// fxdax: the memory the arenas have together, in bytes.
    pub(crate) fn arenas_bytes(&self) -> u64 {
        chunk_bytes(self.locked().given())
    }

    /// fxdax: gives the spare chunks of `arena`, or of every arena, wholly free, back to the node:
    /// memory they hold that nothing uses (TASK-50.3).
    pub(crate) fn release_spares(&self, arena: Option<ArenaId>) {
        self.locked().release_spares(&self.pages, arena);
    }

    /// fxdax: sets whether arenas keep spare chunks; those they have go back if not.
    pub(crate) fn keep_spare_chunks(&self, keep: bool) {
        self.locked().keep_spares(&self.pages, keep);
    }

    /// fxdax: a new arena of physical memory (see `physmap`), which holds no page.
    pub(crate) fn create_arena(&self) -> ArenaId {
        self.locked().create(&self.pages)
    }

    /// fxdax: removes `arena`, not the default: now, if none of its pages is allocated; or once the
    /// last of them is freed, and its chunk goes back. Its allocations fail from now on, whether or
    /// not they may wait (TASK-79.36).
    pub(crate) fn remove_arena(&self, arena: ArenaId) {
        // Under the lock pages are freed under, so that none goes back between the arena's chunks
        // counted and it retiring.
        self.locked().remove(&self.pages, arena);
        // Its allocations waiting for room fail now.
        self.room.pulse();
    }

    /// Frees `frame`'s page, which its holder gave up, back to this node: fxdax: unless
    /// quarantined, as a second stage may map it still.
    pub(crate) fn free(&self, frame: u32) {
        if quarantine::sift(&self.pages.frames, frame) {
            return;
        }
        self.free_locked(self.locked(), &[frame]);
    }

    /// fxdax: counts `frames`' pages, freed, quarantined in their arenas, as relief finds them
    /// (`Arenas::relieved`, TASK-106).
    pub(crate) fn quarantining(&self, frames: &[u32]) {
        let mut locked = self.locked();
        for &frame in frames {
            locked.quarantine(&self.pages, frame, true);
        }
    }

    /// fxdax: frees the pages of `frames`, quarantined, now that every second stage has made the
    /// changes taking them away: as `free` does, as their arenas count them back, under the lock
    /// relief finds them under. They were quarantined, counted (`quarantining`).
    pub(crate) fn free_quarantined(&self, frames: &[u32]) {
        let mut locked = self.locked();
        for &frame in frames {
            locked.quarantine(&self.pages, frame, false);
        }
        self.free_locked(locked, frames);
    }

    /// Frees the pages of `frames` back to this node, under the lock, `locked`: on their arenas'
    /// free lists, at the front, in order, as a list spliced to the front of a free list would be.
    fn free_locked(&self, mut locked: NodeGuard<'_>, frames: &[u32]) {
        let mut room = false;
        for &frame in frames.iter().rev() {
            self.pages.frames.free(frame);
            room |= locked.put(&self.pages, frame);
        }
        self.free_count.fetch_add(frames.len() as u64, Ordering::Relaxed);
        if room {
            self.room.pulse();
        }
    }

    /// fxdax: the number of chunks no arena has.
    pub(crate) fn count_free_chunks(&self) -> usize {
        self.locked().free_chunk_count()
    }

    /// Return count of unallocated physical pages in this node.
    pub(crate) fn count_free_pages(&self) -> u64 {
        self.free_count.load(Ordering::Relaxed)
    }

    /// Return amount of physical memory in this node, in bytes.
    pub(crate) fn count_total_bytes(&self) -> u64 {
        self.pages.memory.size() as u64
    }

    /// Returns whether an allocation has failed as memory is out.
    pub(crate) fn has_alloc_failed_no_mem(&self) -> bool {
        self.alloc_failed_no_mem.load(Ordering::Relaxed)
    }

    /// fxdax: what an allocation fails with that the host mapped no chunk for
    /// (`Refused::Unmapped`, TASK-79.37): `NoMemory` at once, whether or not it may wait, as
    /// nothing waits for the host; reported as the node's failure, as its memory is out as far as
    /// the host goes.
    fn unmapped_status(&self) -> Failed {
        self.report_alloc_failure();
        Failed::NoMemory
    }

    /// Records that an allocation failed as memory is out.
    fn report_alloc_failure(&self) {
        self.alloc_failed_no_mem.store(true, Ordering::Relaxed);
    }

    fn decrement_free_count(&self) {
        let before = self.free_count.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(before >= 1, "a page allocated, none free");
    }
}

/// Returns the PMM node, of physical memory's size ([`crate::memory_bytes`]), creating it on first
/// use.
pub(crate) fn node() -> &'static PmmNode {
    static NODE: OnceLock<PmmNode> = OnceLock::new();
    NODE.get_or_init(|| PmmNode::new(crate::memory_bytes() as usize))
}

/// fxdax: `bytes` of memory, in whole chunks.
fn chunks_in(bytes: u64) -> usize {
    (bytes / CHUNK_SIZE as u64) as usize
}

/// fxdax: the bytes of `chunks` chunks.
fn chunk_bytes(chunks: usize) -> u64 {
    chunks as u64 * CHUNK_SIZE as u64
}
