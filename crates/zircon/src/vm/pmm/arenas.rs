// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: the PMM's arenas, which its free pages are of (TASK-43). An arena takes physical
//! memory a chunk (`physmap::CHUNK_SIZE`) at a time, and its free list holds the free pages of the
//! chunks it has; a chunk whose pages are all free goes back, its memory to the host. An arena
//! also keeps a spare chunk, its pages last on its free list and faulted in from the host ahead of
//! their use (TASK-26.18.3). An arena may be capped, and the arenas together, in chunks
//! (TASK-50.3). One with no free page takes a chunk if one is free and it is within the caps as
//! they stand ([`Arenas::within_caps`]); or else, within its own cap, once enough of other arenas'
//! spares have gone back to free one and bring the arenas under the total cap with it
//! (`spares_wanted`). One that can do neither is starved ([`Arenas::starved`]): its allocations
//! fail, or wait for room, which relief (`pmm::relief`) makes it.
//!
//! [`Arenas`] keeps them, under the PMM's lock: which chunks each arena has, and when it goes, is
//! decided here alone, and `physmap` holds their objects and maps their chunks as it is told, or
//! fails to as the host maps no more (TASK-79.37): a chunk it does not map stays free, and one it
//! does not take back stays its arena's. Once it refuses one, arenas take no spare, which it would
//! refuse too, until it maps or takes back a chunk again; and its refusals meanwhile go unlogged.
//!
//! A chunk given back goes back as the lock is dropped ([`Returning`], TASK-92.35): the host
//! reserving its addresses again and freeing its memory are system calls, which would hold every
//! allocation of every arena's up for as long under the lock, and which no allocation waits for
//! outside it, but one that would take the chunk.

use super::PmmPages;
use crate::sync::rules;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::physmap::{ArenaId, CHUNK_SIZE};
use std::collections::BTreeMap;
use std::ops::Range;

/// The pages of a chunk.
const CHUNK_PAGES: usize = CHUNK_SIZE / page::SIZE;

/// The indices of chunk `chunk`'s pages.
pub(super) fn chunk_pages(chunk: usize) -> Range<usize> {
    chunk * CHUNK_PAGES..(chunk + 1) * CHUNK_PAGES
}

/// Why a starved arena gives no page ([`Arenas::starved`]): the cap it is held to, or its removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Starved {
    /// Its own: pages of its own freed make it room.
    Arena,
    /// The arenas' together, or the node's memory: pages of any arena's freed make it room.
    Total,
    /// It was removed (TASK-79.36): nothing makes it room, and its allocations fail.
    Removed,
}

/// Why an arena gave no page ([`Arenas::take`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refused {
    /// It is starved, held to the cap it says ([`Arenas::starved`]).
    Starved(Starved),
    /// The host mapped no chunk for it (`PhysMemory::assign_chunk`), or took back no spare of
    /// another arena's for it (`PhysMemory::release_chunk`), as the process makes as many mappings
    /// as it may (TASK-79.37): nothing waits for the host, and its allocation fails.
    Unmapped,
    /// Chunks going back would make it room, as the host takes them back outside the PMM's lock
    /// (TASK-92.35): its allocation waits for them, and tries again, as it waited for the lock
    /// while the host took them back under it.
    GoingBack,
}

impl From<Starved> for Refused {
    fn from(starved: Starved) -> Self {
        Self::Starved(starved)
    }
}

/// What relief (`pmm::relief`) has of an arena.
#[derive(Default)]
struct Relief {
    /// The cap it was held to as its allocations last asked for relief, until relief's next pass
    /// begins for it.
    asked: Option<Starved>,
    /// Whether relief's last pass for it freed nothing while it was starved: its allocations fail
    /// rather than wait for ever, until a page of it is freed, a cap changes, or a pass, which
    /// each of them still asks for, frees something.
    exhausted: bool,
}

/// An arena's pages, and what holds it to them.
struct ArenaPages {
    /// The free pages of the chunks it has.
    free: VmPageDoublyLinkedList,
    /// Its spare chunk, none of whose pages is taken yet.
    spare: Option<usize>,
    /// How many chunks it has, its spare and those going back included (TASK-50.3).
    chunks: usize,
    /// How many of its chunks are going back: given back, and not yet taken back by the host
    /// (TASK-92.35).
    going_back: usize,
    /// The most chunks it may have, if capped.
    cap: Option<usize>,
    relief: Relief,
    /// fxdax: its pages freed and quarantined, as a second stage may map them still, which come
    /// back to it once every second stage has made the changes taking them away
    /// (`pmm::quarantine`): relief exhausts it not while any is to come back, as each makes it room
    /// (TASK-106).
    quarantined: usize,
    /// Whether it was removed, and goes with its last chunk: it gives no page meanwhile.
    retiring: bool,
}

impl ArenaPages {
    fn new() -> Self {
        Self {
            free: VmPageDoublyLinkedList::new(),
            spare: None,
            chunks: 0,
            going_back: 0,
            cap: None,
            relief: Relief::default(),
            quarantined: 0,
            retiring: false,
        }
    }

    /// Whether one more chunk keeps it within its own cap.
    fn within_cap(&self) -> bool {
        self.cap.is_none_or(|cap| self.chunks < cap)
    }
}

/// The PMM's arenas, and the chunks none has.
///
/// - A chunk is free, which no arena has and which is mapped nowhere, or one arena's: physmap's
///   chunk table names the arena, which counts it among its `chunks`; but for a chunk going back,
///   which its arena counts, and the table names none once the host has taken it back, until it
///   is free (TASK-92.35).
/// - A chunk's free pages are on its arena's free list, and `chunk_used` counts the others. One
///   none of whose pages is allocated goes back, unless it is its arena's spare: off the free
///   list, it is going back until the host takes it back, as the PMM's lock is dropped
///   ([`Arenas::take_returning`]), or at once, another arena's spare given back for a chunk an
///   allocation takes. Unless the host does not take it back (TASK-79.37): it is its arena's
///   spare then, if its arena has none, which goes back with the spares, whether arenas keep them
///   or not; or else it goes back once its pages are freed again.
/// - A spare's pages are all on its arena's free list, none of them taken yet: an arena whose free
///   list is empty has no spare.
/// - An arena is here from its creation until its removal, or the release of its last chunk after
///   that, finds it holding no chunk; and its object is in physmap as long. Removed, here still or
///   gone, it gives no page, of its chunks' free pages or of a chunk it would take.
/// - An arena is starved exactly when taking a page of it fails, held to the same cap, as both
///   count the spares it wants (`spares_wanted`), or removed; and neither while chunks going back
///   would make it room, which taking a page of it waits for (`Refused::GoingBack`). Spares given
///   back starve no arena less, as they lower the spares wanted as much as the spares there are:
///   no allocation waits for them, but for their going back.
/// - Relief is `asked` for an arena from an allocation's asking until its next pass begins; and
///   the arena is `exhausted` from a pass that freed nothing while it was starved until a page of
///   it is freed, a cap changes, or a pass frees something.
/// - The host refuses chunks from its refusing to map one, or take one back, until it next does as
///   asked (TASK-79.37): meanwhile no arena takes a spare.
pub(super) struct Arenas {
    /// The arenas, by id.
    arenas: BTreeMap<ArenaId, ArenaPages>,
    /// The id of the next arena created.
    next: u32,
    /// The chunks no arena has, whose pages are all free: the last given back is taken first.
    free_chunks: Vec<usize>,
    /// The chunks given back under the PMM's lock held now, for the host to take back once it is
    /// dropped ([`Arenas::take_returning`]).
    returning: Vec<Returning>,
    /// How many pages of each chunk are allocated.
    chunk_used: Box<[u32]>,
    /// The most chunks the arenas may have together, if capped.
    total_cap: Option<usize>,
    /// Whether arenas keep spare chunks.
    keep_spares: bool,
    /// Whether the host refused the last chunk it was asked to map, or take back, as the process
    /// makes as many mappings as it may (TASK-79.37). Meanwhile arenas take no spare, which it
    /// would refuse too, each allocation asking it in vain under the PMM's lock; an allocation that
    /// has no page but of a chunk still asks it for one.
    host_refuses: bool,
}

impl Arenas {
    /// The default arena alone, which holds none of the node's `chunks` chunks yet.
    pub(super) fn new(chunks: usize) -> Self {
        Self {
            arenas: BTreeMap::from([(ArenaId::DEFAULT, ArenaPages::new())]),
            next: ArenaId::DEFAULT.0 + 1,
            // Lower chunks first.
            free_chunks: (0..chunks).rev().collect(),
            returning: Vec::new(),
            chunk_used: vec![0; chunks].into(),
            total_cap: None,
            keep_spares: true,
            host_refuses: false,
        }
    }

    /// A new arena, which holds no chunk, and its object.
    pub(super) fn create(&mut self, pages: &PmmPages) -> ArenaId {
        let arena = ArenaId(self.next);
        self.next += 1;
        pages.memory.create_arena(arena);
        self.arenas.insert(arena, ArenaPages::new());
        arena
    }

    /// Removes `arena`, not the default: it gives no page from now on ([`Starved::Removed`]), its
    /// spare chunk goes back if the host takes it, and its cap, which nothing is held to now, is
    /// lifted; and it goes, with its object, now if it holds no chunk, or else with its last chunk,
    /// as its spare goes back, and its pages still allocated, those of objects dropped meanwhile,
    /// are freed. One gone is let be.
    pub(super) fn remove(&mut self, pages: &PmmPages, arena: ArenaId) {
        assert_ne!(arena, ArenaId::DEFAULT, "the default arena stays");
        self.release_spares(pages, Some(arena));
        let Some(entry) = self.arenas.get_mut(&arena) else { return };
        entry.cap = None;
        if entry.chunks > 0 {
            entry.retiring = true;
        } else {
            self.arenas.remove(&arena);
            pages.memory.remove_arena(arena);
        }
    }

    /// A free page of `arena`'s, taking a chunk for it if it has none: one no arena has, once as
    /// many of other arenas' spares as it wants have gone back for it (`spares_wanted`). Once it
    /// starts on its spare, it takes another, if one is free and it is within the caps as they
    /// stand ([`Arenas::within_caps`]): no spare goes back for a spare, and it does without one
    /// the host does not map, or would not, refusing chunks. It fails as [`Arenas::starved`] says,
    /// having given back what spares there were; or as the host does not map the chunk it takes,
    /// or take back a spare for it. It waits, rather than take a spare, or fail, while chunks going
    /// back would make it room (`Refused::GoingBack`).
    pub(super) fn take(&mut self, pages: &PmmPages, arena: ArenaId) -> Result<VmPagePtr, Refused> {
        let entry = self.live(arena).ok_or(Starved::Removed)?;
        if entry.free.is_empty() {
            debug_assert!(entry.spare.is_none(), "{arena:?} has a spare, and no free page");
            let wanted = self.spares_wanted(arena);
            if wanted != Ok(0) && self.waits_for_chunks_going_back(arena) {
                return Err(Refused::GoingBack);
            }
            for _ in 0..wanted? {
                let spare = self.take_spare(|other| other != arena).ok_or(Starved::Total)?;
                if !self.release(pages, spare) {
                    return Err(Refused::Unmapped);
                }
            }
            self.give(pages, arena).ok_or(Refused::Unmapped)?;
        }
        let page = self.entry_mut(arena).free.pop_front().expect("a free page of the arena's");
        self.taken(pages, arena, pages.memory.chunk_of(page.paddr()));
        Ok(page)
    }

    /// fxdax: the page after `page`, physically, if it is free, on its arena's free list, and of
    /// the same chunk, taken as [`Arenas::take`] takes one: so that the pages an allocation takes
    /// together are consecutive where they can be, as a window's second stage maps each run of
    /// them with a mapping of its own, not a page each. Taken a page at a time from the list, as
    /// evictions refill it in the order they free them, an extent's pages are scattered more with
    /// each eviction, and the window's runs shorter (TASK-108.1). The caller marks each page it
    /// takes allocated, under the lock that it took it under, so that one free and listed is on
    /// the free list.
    pub(super) fn take_after(&mut self, pages: &PmmPages, page: VmPagePtr) -> Option<VmPagePtr> {
        let chunk = pages.memory.chunk_of(page.paddr());
        let index = pages.index_of(page)? + 1;
        if index >= chunk_pages(chunk).end {
            return None;
        }
        let next = pages.page(index);
        // SAFETY: Under the node's lock, which guards the free lists, the pages' membership of.
        if !next.is_free() || !unsafe { next.in_container() } {
            return None;
        }
        let arena = pages.memory.chunk_arena(chunk)?;
        self.live(arena)?;
        self.entry_mut(arena).free.erase(next);
        self.taken(pages, arena, chunk);
        Some(next)
    }

    /// A page of `arena`'s, of `chunk`, was taken off its free list: the chunk is its spare no
    /// more, and it takes another, if it may, as [`Arenas::take`] says.
    fn taken(&mut self, pages: &PmmPages, arena: ArenaId, chunk: usize) {
        let entry = self.entry_mut(arena);
        if entry.spare == Some(chunk) {
            entry.spare = None;
        }
        if self.keep_spares
            && !self.host_refuses
            && self.entry(arena).spare.is_none()
            && self.within_caps(arena)
            && !self.free_chunks.is_empty()
            && let Some(spare) = self.give(pages, arena)
        {
            self.entry_mut(arena).spare = Some(spare);
        }
        self.chunk_used[chunk] += 1;
    }

    /// Puts `page`, free, back on its arena's free list, at the front unless `delay_reuse`, and
    /// its chunk back if the page was the last of it allocated, as the lock is dropped
    /// ([`Arenas::take_returning`]). Returns whether an allocation waiting for room may find it
    /// now: as the arena is at a cap ([`Arenas::within_caps`]), its own allocations waiting for its
    /// pages; or as no chunk is free, every arena's waiting for pages freed until one goes back,
    /// and trying again as each is, which, without relief, asks the evictor for more. A chunk
    /// going back makes room once it is back ([`Arenas::returned`]).
    #[must_use]
    pub(super) fn put(&mut self, pages: &PmmPages, page: VmPagePtr, delay_reuse: bool) -> bool {
        let chunk = pages.memory.chunk_of(page.paddr());
        let arena = pages.memory.chunk_arena(chunk).expect("a page of no arena's");
        let entry = self.entry_mut(arena);
        if delay_reuse {
            entry.free.push_back(page);
        } else {
            entry.free.push_front(page);
        }
        entry.relief.exhausted = false;
        self.chunk_used[chunk] -= 1;
        if self.chunk_used[chunk] == 0 {
            let returning = self.give_back(pages, chunk);
            self.returning.push(returning);
            return false;
        }
        !self.within_caps(arena) || self.free_chunks.is_empty()
    }

    /// fxdax: `page`, freed, is quarantined, to come back to its arena ([`Arenas::put`]) once every
    /// second stage has made the changes taking it away; or, `quarantined` false, it is coming
    /// back now (TASK-106).
    pub(super) fn quarantine(&mut self, pages: &PmmPages, page: VmPagePtr, quarantined: bool) {
        let chunk = pages.memory.chunk_of(page.paddr());
        let arena = pages.memory.chunk_arena(chunk).expect("a page of no arena's");
        let entry = self.entry_mut(arena);
        match quarantined {
            true => entry.quarantined += 1,
            false => {
                entry.quarantined = (entry.quarantined.checked_sub(1))
                    .expect("a page came back from quarantine its arena didn't count");
            }
        }
    }

    /// The cap `arena` is held to, if it is starved: it has no free page, and can take no chunk,
    /// even with other arenas' spares gone back for one: past its own cap, or, past the total's or
    /// out of chunks, with fewer of their spares than it wants (`spares_wanted`); and no chunk
    /// going back would make it room (TASK-92.35). An arena removed, here still or gone, is starved
    /// as removed.
    pub(super) fn starved(&self, arena: ArenaId) -> Option<Starved> {
        let Some(entry) = self.live(arena) else { return Some(Starved::Removed) };
        if !entry.free.is_empty() {
            return None;
        }
        match self.spares_wanted(arena) {
            _ if self.waits_for_chunks_going_back(arena) => None,
            Ok(wanted) => (wanted > self.spares()).then_some(Starved::Total),
            Err(starved) => Some(starved),
        }
    }

    /// Whether one more chunk keeps `arena` within its cap, and the arenas within theirs together
    /// (TASK-50.3), as they stand: whether it may take a chunk with no spare going back for it, as
    /// it takes a spare. At the total cap it is not, though other arenas' spares going back may
    /// still make it room for one (`spares_wanted`): whether it can take no chunk at all is
    /// [`Arenas::starved`]'s to say. An arena gone has no cap of its own.
    pub(super) fn within_caps(&self, arena: ArenaId) -> bool {
        self.arenas.get(&arena).is_none_or(ArenaPages::within_cap)
            && self.total_cap.is_none_or(|cap| self.given() < cap)
    }

    /// Caps the chunks `arena` may have, or lifts its cap. Lowered under what it has, it takes no
    /// more until it has less. With a cap changed, no arena is exhausted. An arena gone has none.
    pub(super) fn set_cap(&mut self, arena: ArenaId, cap: Option<usize>) {
        let Some(entry) = self.arenas.get_mut(&arena) else { return };
        entry.cap = cap;
        self.clear_exhausted();
    }

    /// Caps the chunks the arenas may have together, or lifts the cap. With it changed, no arena
    /// is exhausted.
    pub(super) fn set_total_cap(&mut self, cap: Option<usize>) {
        self.total_cap = cap;
        self.clear_exhausted();
    }

    /// The most chunks the arenas may have together, if capped.
    pub(super) fn total_cap(&self) -> Option<usize> {
        self.total_cap
    }

    /// An allocation of `arena`'s, starved and held to `starved`, asks for relief, which relief's
    /// next pass gives the arena at that cap ([`Arenas::take_asked`]). Returns whether the
    /// allocation fails rather than waits for it: the arena is exhausted, or, gone, has nothing to
    /// relieve.
    pub(super) fn ask_relief(&mut self, arena: ArenaId, starved: Starved) -> bool {
        let Some(entry) = self.arenas.get_mut(&arena) else { return true };
        entry.relief.asked = Some(starved);
        entry.relief.exhausted
    }

    /// The arenas relief was asked for since its last pass, in order, each with the cap it was
    /// last held to: what its next pass relieves. Those asked for from now on, the pass after.
    pub(super) fn take_asked(&mut self) -> Vec<(ArenaId, Starved)> {
        (self.arenas.iter_mut())
            .filter_map(|(&arena, entry)| Some((arena, entry.relief.asked.take()?)))
            .collect()
    }

    /// Relief's pass for `arena` freed `freed` bytes: something, and it is not exhausted; nothing,
    /// and it is, if it is starved still, but while pages of it are quarantined, which make it room
    /// as they come back (TASK-106): what relief evicted, a window mapped, waits for its VM process
    /// to unmap it, and a pass after finds nothing left to evict.
    pub(super) fn relieved(&mut self, arena: ArenaId, freed: u64) {
        let starved = self.starved(arena).is_some();
        let Some(entry) = self.arenas.get_mut(&arena) else { return };
        if freed > 0 {
            entry.relief.exhausted = false;
        } else if starved && entry.quarantined == 0 {
            entry.relief.exhausted = true;
        }
    }

    /// Gives the spare chunks of `arena`, or of every arena, back, as the lock is dropped
    /// ([`Arenas::take_returning`]): those the host takes back.
    pub(super) fn release_spares(&mut self, pages: &PmmPages, arena: Option<ArenaId>) {
        let spares: Vec<usize> = (self.arenas.iter_mut())
            .filter(|(of, _)| arena.is_none_or(|arena| **of == arena))
            .filter_map(|(_, entry)| entry.spare.take())
            .collect();
        for spare in spares {
            let returning = self.give_back(pages, spare);
            self.returning.push(returning);
        }
    }

    /// The chunks given back under the PMM's lock held now, for the host to take back once it is
    /// dropped, outside it ([`Returning::release`]): their arenas have them, going back, until
    /// [`Arenas::returned`] says what the host did.
    #[must_use]
    pub(super) fn take_returning(&mut self) -> Vec<Returning> {
        std::mem::take(&mut self.returning)
    }

    /// Records what the host did with a chunk going back ([`Returning::release`]). Taken back, it
    /// is free, and an arena removed goes with its last chunk, and its object. Not, as the host does
    /// not reserve it again (TASK-79.37), it stays its arena's, its pages free, as its spare if it
    /// has none. Either way allocations waiting for room may find it now, which the caller tells
    /// them.
    pub(super) fn returned(&mut self, pages: &PmmPages, returned: Returned) {
        let Returned { chunk, arena, result } = returned;
        let what =
            format_args!("chunk {chunk} of {arena:?} kept, as the host did not reserve it again");
        self.host_did(result.as_ref().err(), what);
        let entry = self.arenas.get_mut(&arena).expect("the arena of a chunk going back");
        entry.going_back -= 1;
        if result.is_err() {
            for index in chunk_pages(chunk) {
                entry.free.push_back(pages.page(index));
            }
            entry.spare.get_or_insert(chunk);
            return;
        }
        self.free_chunks.push(chunk);
        entry.chunks -= 1;
        if entry.retiring && entry.chunks == 0 {
            self.arenas.remove(&arena);
            pages.memory.remove_arena(arena);
        }
    }

    /// Sets whether arenas keep spare chunks; those they have go back if not.
    pub(super) fn keep_spares(&mut self, pages: &PmmPages, keep: bool) {
        self.keep_spares = keep;
        if !keep {
            self.release_spares(pages, None);
        }
    }

    /// How many chunks `arena` has, its spare included: none, once it has gone.
    pub(super) fn chunks(&self, arena: ArenaId) -> usize {
        self.arenas.get(&arena).map_or(0, |entry| entry.chunks)
    }

    /// How many chunks the arenas have together.
    pub(super) fn given(&self) -> usize {
        self.chunk_used.len() - self.free_chunks.len()
    }

    /// How many chunks no arena has.
    pub(super) fn free_chunk_count(&self) -> usize {
        self.free_chunks.len()
    }

    /// How many of other arenas' spares must go back for `arena`, which has no free page, to take
    /// a chunk: enough for the arenas to be within the total cap with it, and one if no chunk is
    /// free. `Starved::Arena` past its own cap, which no spare helps.
    fn spares_wanted(&self, arena: ArenaId) -> Result<usize, Starved> {
        if !self.entry(arena).within_cap() {
            return Err(Starved::Arena);
        }
        let over_total = self.total_cap.map_or(0, |cap| (self.given() + 1).saturating_sub(cap));
        Ok(over_total.max(usize::from(self.free_chunks.is_empty())))
    }

    /// Whether chunks going back would make `arena`, which has no free page, room to take one: one
    /// of its own, which it then has no more, or, within its own cap, any, which is free then
    /// (TASK-92.35). Its allocations wait for them rather than take another arena's spare, or fail,
    /// as they waited for the PMM's lock while the host took them back under it.
    fn waits_for_chunks_going_back(&self, arena: ArenaId) -> bool {
        let entry = self.entry(arena);
        rules().allocations_wait_for_chunks_going_back
            && (entry.going_back > 0
                || entry.within_cap() && self.arenas.values().any(|other| other.going_back > 0))
    }

    /// How many arenas have a spare chunk.
    fn spares(&self) -> usize {
        self.arenas.values().filter(|entry| entry.spare.is_some()).count()
    }

    /// The pages of `arena`, unless it was removed.
    fn live(&self, arena: ArenaId) -> Option<&ArenaPages> {
        self.arenas.get(&arena).filter(|entry| !entry.retiring)
    }

    fn entry(&self, arena: ArenaId) -> &ArenaPages {
        self.arenas.get(&arena).unwrap_or_else(|| panic!("no arena {arena:?}"))
    }

    fn entry_mut(&mut self, arena: ArenaId) -> &mut ArenaPages {
        self.arenas.get_mut(&arena).unwrap_or_else(|| panic!("no arena {arena:?}"))
    }

    fn clear_exhausted(&mut self) {
        self.arenas.values_mut().for_each(|entry| entry.relief.exhausted = false);
    }

    /// Gives the free chunk given back last to `arena`, its pages last on its free list, faulted in
    /// ahead of their use, and returns it; or leaves it free, as the host does not map it
    /// (`PhysMemory::assign_chunk`).
    fn give(&mut self, pages: &PmmPages, arena: ArenaId) -> Option<usize> {
        let &chunk = self.free_chunks.last().expect("a free chunk");
        let assigned = pages.memory.assign_chunk(chunk, arena);
        let what = format_args!("no chunk for {arena:?}, as the host did not map one");
        self.host_did(assigned.as_ref().err(), what);
        assigned.ok()?;
        self.free_chunks.pop();
        let entry = self.entry_mut(arena);
        entry.chunks += 1;
        for index in chunk_pages(chunk) {
            entry.free.push_back(pages.page(index));
        }
        pages.memory.populate_later(chunk);
        Some(chunk)
    }

    /// Records what the host did as it was asked to map, or take back, a chunk (TASK-79.37):
    /// refused, with the error `refusal`, or did as asked. A refusal is logged, saying `what` came
    /// of it, as the host begins refusing chunks; those that follow are not, until it has done as
    /// asked again, as an arena with no page but of a chunk asks for one at each allocation.
    fn host_did(&mut self, refusal: Option<&std::io::Error>, what: std::fmt::Arguments<'_>) {
        let refused = std::mem::replace(&mut self.host_refuses, refusal.is_some());
        if let Some(error) = refusal
            && !refused
        {
            eprintln!(
                "WARNING: PMM: {what}: {error}; no arena takes a spare until the host maps, or \
                 takes back, a chunk"
            );
        }
    }

    /// Takes the spare chunk of the first arena `which` accepts that has one from it, to give it
    /// back.
    fn take_spare(&mut self, which: impl Fn(ArenaId) -> bool) -> Option<usize> {
        (self.arenas.iter_mut())
            .find(|(arena, entry)| entry.spare.is_some() && which(**arena))
            .and_then(|(_, entry)| entry.spare.take())
    }

    /// Gives `chunk`, none of whose pages is allocated, back at once, under the lock: another
    /// arena's spare, for a chunk an allocation takes now. Returns whether the host took it back
    /// ([`Arenas::returned`]).
    fn release(&mut self, pages: &PmmPages, chunk: usize) -> bool {
        let returned = self.give_back(pages, chunk).release(pages);
        let taken = returned.result.is_ok();
        self.returned(pages, returned);
        taken
    }

    /// Gives `chunk`, none of whose pages is allocated, back: off its arena's free list, which has
    /// it, going back, until the host takes it back ([`Returning::release`]).
    fn give_back(&mut self, pages: &PmmPages, chunk: usize) -> Returning {
        let arena = pages.memory.chunk_arena(chunk).expect("a chunk of no arena's");
        debug_assert!(
            chunk_pages(chunk).all(|index| pages.page(index).is_free()),
            "chunk {chunk} of {arena:?} goes back with a page allocated"
        );
        let entry = self.entry_mut(arena);
        for index in chunk_pages(chunk) {
            entry.free.erase(pages.page(index));
        }
        entry.going_back += 1;
        Returning { chunk, arena }
    }
}

/// A chunk going back: given back under the PMM's lock, for the host to take back outside it
/// (TASK-92.35).
#[must_use = "the host takes the chunk back"]
pub(super) struct Returning {
    chunk: usize,
    arena: ArenaId,
}

impl Returning {
    /// Has the host take the chunk back, without the PMM's lock: reserve its addresses again, and
    /// free its memory (`PhysMemory::release_chunk`). What it did is for [`Arenas::returned`], under
    /// the lock again.
    pub(super) fn release(self, pages: &PmmPages) -> Returned {
        let Self { chunk, arena } = self;
        Returned { chunk, arena, result: pages.memory.release_chunk(chunk) }
    }
}

/// A chunk going back the host was asked to take back ([`Returning::release`]), and whether it did.
#[must_use = "the arenas record what the host did"]
pub(super) struct Returned {
    chunk: usize,
    arena: ArenaId,
    result: std::io::Result<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunks of the tests' physical memory.
    const CHUNKS: usize = 16;

    /// Arenas over physical memory of their own, no node's. The arenas go first, their free lists
    /// pointing into the pages.
    struct Fixture {
        arenas: Arenas,
        pages: PmmPages,
    }

    impl Fixture {
        fn new() -> Self {
            let pages = PmmPages::new(CHUNKS * CHUNK_SIZE);
            Self { arenas: Arenas::new(pages.memory.chunk_count()), pages }
        }

        fn create(&mut self) -> ArenaId {
            self.arenas.create(&self.pages)
        }

        fn take(&mut self, arena: ArenaId) -> Result<VmPagePtr, Refused> {
            self.arenas.take(&self.pages, arena)
        }

        /// As many pages of `arena`'s as `chunks` chunks hold.
        fn take_chunks(&mut self, arena: ArenaId, chunks: usize) -> Vec<VmPagePtr> {
            (0..chunks * CHUNK_PAGES).map(|_| self.take(arena).unwrap()).collect()
        }

        /// A page of each of `arenas`'.
        fn take_each(&mut self, arenas: &[ArenaId]) -> Vec<VmPagePtr> {
            arenas.iter().map(|&arena| self.take(arena).unwrap()).collect()
        }

        /// Puts `page` back, as the node frees it: returns whether allocations waiting are told of
        /// room, as they are when the page's chunk goes back, once it is back.
        fn put(&mut self, page: VmPagePtr) -> bool {
            let room = self.arenas.put(&self.pages, page, false);
            self.return_chunks() || room
        }

        fn remove(&mut self, arena: ArenaId) {
            self.arenas.remove(&self.pages, arena);
            self.return_chunks();
        }

        fn keep_spares(&mut self, keep: bool) {
            self.arenas.keep_spares(&self.pages, keep);
            self.return_chunks();
        }

        fn release_spares(&mut self) {
            self.arenas.release_spares(&self.pages, None);
            self.return_chunks();
        }

        /// Has the host take back the chunks given back, as the node does as its lock is dropped
        /// (`NodeGuard`): returns whether any went back.
        fn return_chunks(&mut self) -> bool {
            let returning = self.arenas.take_returning();
            let any = !returning.is_empty();
            for chunk in returning {
                let returned = chunk.release(&self.pages);
                self.arenas.returned(&self.pages, returned);
            }
            any
        }

        /// Has the host refuse to take back the chunks given back, as it does when reserving one
        /// again would split a mapping, and the process makes as many as it may (TASK-79.37): they
        /// stay mapped as they were.
        fn refuse_chunks(&mut self) {
            for Returning { chunk, arena } in self.arenas.take_returning() {
                let result = Err(std::io::Error::from_raw_os_error(libc::ENOMEM));
                self.arenas.returned(&self.pages, Returned { chunk, arena, result });
            }
        }

        /// Puts the pages `taken` back.
        fn put_each(&mut self, taken: Vec<VmPagePtr>) {
            for page in taken {
                let _ = self.put(page);
            }
        }

        /// Puts the pages `taken` back, and the spares: then no chunk is any arena's.
        fn put_all(&mut self, taken: Vec<VmPagePtr>) {
            self.put_each(taken);
            self.release_spares();
            assert_eq!(self.arenas.given(), 0, "a chunk is still an arena's");
        }
    }

    /// Each chunk is free, or one arena's, as physmap's chunk table says: an arena has the chunks
    /// the table names it for, and the arenas together those not free.
    #[test]
    fn arenas_have_the_chunks_physmap_names_them_for() {
        let mut fixture = Fixture::new();
        let (first, second) = (fixture.create(), fixture.create());
        // Two chunks' worth of the first's, which has a spare after each: three.
        let mut taken = fixture.take_chunks(first, 2);
        // A chunk, and a spare.
        taken.extend(fixture.take_each(&[second]));
        assert_eq!(fixture.arenas.chunks(first), 3);
        assert_eq!(fixture.arenas.chunks(second), 2);
        assert_eq!(fixture.arenas.given(), 5);
        assert_eq!(fixture.arenas.free_chunk_count(), CHUNKS - 5);
        let memory = &fixture.pages.memory;
        for arena in [ArenaId::DEFAULT, first, second] {
            let named = (0..CHUNKS).filter(|&chunk| memory.chunk_arena(chunk) == Some(arena));
            assert_eq!(named.count(), fixture.arenas.chunks(arena), "{arena:?}");
        }
        fixture.put_all(taken);
        let memory = &fixture.pages.memory;
        assert_eq!((0..CHUNKS).find(|&chunk| memory.chunk_arena(chunk).is_some()), None);
    }

    /// An arena removed gives its spare back, and its cap is lifted. Holding a page, it stays, and
    /// its object, until the page's chunk goes back; one holding none goes at once. Either is
    /// starved as removed, and gives no page, though one holding a page has free pages and chunks
    /// are free (TASK-79.36). Removing an arena again does nothing.
    #[test]
    fn removed_arenas_go_with_their_last_chunk() {
        let mut fixture = Fixture::new();
        let (empty, holding) = (fixture.create(), fixture.create());
        fixture.remove(empty);
        assert!(fixture.pages.memory.arena_fd(empty).is_none());
        assert_eq!(fixture.arenas.starved(empty), Some(Starved::Removed));
        assert_eq!(fixture.take(empty).err(), Some(Starved::Removed.into()));

        let page = fixture.take(holding).unwrap();
        assert_eq!(fixture.arenas.chunks(holding), 2, "a chunk, and a spare");
        // Capped under what it has.
        fixture.arenas.set_cap(holding, Some(1));
        assert!(!fixture.arenas.within_caps(holding));
        fixture.remove(holding);
        assert_eq!(fixture.arenas.chunks(holding), 1, "its spare went back");
        assert!(fixture.arenas.within_caps(holding), "its cap was lifted");
        assert!(fixture.pages.memory.arena_fd(holding).is_some());
        assert_eq!(fixture.arenas.starved(holding), Some(Starved::Removed));
        let given = fixture.arenas.given();
        assert_eq!(fixture.take(holding).err(), Some(Starved::Removed.into()));
        assert_eq!(fixture.arenas.given(), given, "a chunk taken");
        fixture.remove(holding);
        fixture.remove(empty);
        assert_eq!(fixture.arenas.chunks(holding), 1);
        assert!(fixture.pages.memory.arena_fd(holding).is_some());

        assert!(fixture.put(page), "its chunk went back");
        assert_eq!(fixture.arenas.chunks(holding), 0);
        assert!(fixture.pages.memory.arena_fd(holding).is_none());
        assert_eq!(fixture.arenas.given(), 0);
    }

    /// Taking a page of an arena fails exactly as it is starved, held to the same cap: past its
    /// own; past the total's, or out of chunks, unless another arena's spare goes back for it,
    /// which it then takes, the arenas holding as many chunks as before, and no spare.
    #[test]
    fn takes_fail_as_starved_says() {
        /// Makes the arena to take a page of, and the pages taken for that.
        type Setup = fn(&mut Fixture) -> (ArenaId, Vec<VmPagePtr>);
        let cases: [(&str, Setup, Option<Starved>, usize); 5] = [
            (
                "at its own cap",
                |fixture| {
                    let arena = fixture.create();
                    fixture.arenas.set_cap(arena, Some(1));
                    (arena, fixture.take_chunks(arena, 1))
                },
                Some(Starved::Arena),
                1,
            ),
            (
                "at the total's, with no spare",
                |fixture| {
                    fixture.arenas.set_total_cap(Some(1));
                    let arena = fixture.create();
                    (arena, fixture.take_chunks(arena, 1))
                },
                Some(Starved::Total),
                1,
            ),
            (
                "at the total's, with another arena's spare",
                |fixture| {
                    let other = fixture.create();
                    let taken = fixture.take_each(&[other]);
                    fixture.arenas.set_total_cap(Some(fixture.arenas.given()));
                    (fixture.create(), taken)
                },
                None,
                1,
            ),
            (
                "out of chunks, with another arena's spare",
                |fixture| {
                    let hog = fixture.create();
                    let taken = fixture.take_chunks(hog, CHUNKS - 1);
                    assert_eq!(fixture.arenas.free_chunk_count(), 0);
                    (fixture.create(), taken)
                },
                None,
                1,
            ),
            (
                "out of chunks, with no spare",
                |fixture| {
                    fixture.keep_spares(false);
                    let hog = fixture.create();
                    let taken = fixture.take_chunks(hog, CHUNKS);
                    (fixture.create(), taken)
                },
                Some(Starved::Total),
                0,
            ),
        ];
        for (case, setup, starved, chunks) in cases {
            let mut fixture = Fixture::new();
            let (arena, mut taken) = setup(&mut fixture);
            let given = fixture.arenas.given();
            assert_eq!(fixture.arenas.starved(arena), starved, "{case}");
            let take = fixture.take(arena);
            assert_eq!(take.err(), starved.map(Refused::Starved), "{case}");
            assert_eq!(fixture.arenas.chunks(arena), chunks, "{case}");
            assert_eq!(fixture.arenas.given(), given, "{case}");
            assert_eq!(fixture.arenas.spares(), 0, "{case}");
            taken.extend(take);
            fixture.put_all(taken);
        }
    }

    /// A total cap lowered under what the arenas hold: an arena takes a chunk once as many spares
    /// as bring the arenas under it with that chunk have gone back, all at once; with too few
    /// spares, it fails, starved still, those there were gone back.
    #[test]
    fn a_total_cap_lowered_under_the_arenas_takes_their_spares() {
        let mut fixture = Fixture::new();
        // A chunk and a spare each.
        let others = [fixture.create(), fixture.create(), fixture.create()];
        let mut taken = fixture.take_each(&others);
        let arena = fixture.create();
        fixture.arenas.set_total_cap(Some(4));
        // Three spares wanted, of three.
        assert_eq!(fixture.arenas.starved(arena), None);
        taken.extend(fixture.take(arena));
        assert_eq!(fixture.arenas.spares(), 0);
        assert_eq!(fixture.arenas.given(), 4);
        fixture.arenas.set_total_cap(None);
        fixture.put_all(taken);

        let mut fixture = Fixture::new();
        let others = [fixture.create(), fixture.create()];
        let taken = fixture.take_each(&others);
        let arena = fixture.create();
        fixture.arenas.set_total_cap(Some(2));
        // Three spares wanted, of two.
        assert_eq!(fixture.arenas.starved(arena), Some(Starved::Total));
        assert_eq!(fixture.take(arena).err(), Some(Starved::Total.into()));
        assert_eq!(fixture.arenas.spares(), 0);
        assert_eq!(fixture.arenas.given(), 2);
        assert_eq!(fixture.arenas.starved(arena), Some(Starved::Total));
        fixture.put_all(taken);
    }

    /// Spares given back leave each arena as starved as it was, or more, never less.
    #[test]
    fn spares_given_back_starve_no_arena_less() {
        // Under a total cap a chunk under what the arenas hold, as many spares wanted as there
        // are: not starved, nor once they have gone back, none wanted then.
        let mut fixture = Fixture::new();
        let others = [fixture.create(), fixture.create()];
        let taken = fixture.take_each(&others);
        let arena = fixture.create();
        fixture.arenas.set_total_cap(Some(3));
        assert_eq!(fixture.arenas.starved(arena), None);
        fixture.release_spares();
        assert_eq!(fixture.arenas.starved(arena), None);
        fixture.put_all(taken);

        // Under one two chunks under, one more wanted than there are: starved, and still once they
        // have gone back, one wanted then, of none.
        let mut fixture = Fixture::new();
        let others = [fixture.create(), fixture.create()];
        let taken = fixture.take_each(&others);
        let arena = fixture.create();
        fixture.arenas.set_total_cap(Some(2));
        assert_eq!(fixture.arenas.starved(arena), Some(Starved::Total));
        fixture.release_spares();
        assert_eq!(fixture.arenas.starved(arena), Some(Starved::Total));
        fixture.put_all(taken);
    }

    /// At the total cap an arena is not within the caps as they stand, nor starved, as another
    /// arena's spare goes back for the chunk it takes; and, not within them still, it takes no
    /// spare of its own.
    #[test]
    fn within_caps_counts_no_spare_going_back() {
        let mut fixture = Fixture::new();
        let other = fixture.create();
        // A chunk, and a spare.
        let mut taken = fixture.take_each(&[other]);
        fixture.arenas.set_total_cap(Some(fixture.arenas.given()));
        let arena = fixture.create();
        assert!(!fixture.arenas.within_caps(arena));
        assert_eq!(fixture.arenas.starved(arena), None);
        taken.extend(fixture.take(arena));
        assert_eq!(fixture.arenas.chunks(arena), 1, "the other's spare, and none of its own");
        assert_eq!(fixture.arenas.spares(), 0);
        assert!(!fixture.arenas.within_caps(arena));
        fixture.put_all(taken);
    }

    /// A page put back tells of room for allocations waiting when its chunk goes back, when its
    /// arena is at a cap, and when no chunk is free; not otherwise.
    #[test]
    fn puts_tell_when_waiters_may_find_room() {
        let mut fixture = Fixture::new();
        let arena = fixture.create();
        // Of one chunk, beside its spare.
        let pages = fixture.take_each(&[arena; 4]);
        let [last, out_of_chunks, capped, uncapped] = pages.try_into().unwrap();
        assert!(!fixture.put(uncapped));
        // Capped under what it has.
        fixture.arenas.set_cap(arena, Some(1));
        assert!(fixture.put(capped));
        fixture.arenas.set_cap(arena, None);
        // The hog takes the chunks left, the last its spare.
        let hog = fixture.create();
        let hogged = fixture.take_chunks(hog, CHUNKS - 3);
        assert_eq!(fixture.arenas.free_chunk_count(), 0);
        assert!(fixture.put(out_of_chunks));
        fixture.put_each(hogged);
        assert!(fixture.put(last), "its chunk went back");
        fixture.put_all(Vec::new());
    }

    /// TASK-92.35: a chunk going back, given back under the PMM's lock and not yet taken back by
    /// the host, is its arena's still, and no chunk is free for it yet; an arena it would make room
    /// for waits for it, rather than fail, or take a spare: one at the total cap, or its own arena
    /// at its own cap. One at its own cap it makes no room for is starved as before. Once the host
    /// has taken it back, it is free, and the arena waiting takes it.
    #[test]
    fn chunks_going_back_are_waited_for_by_the_arenas_they_make_room_for() {
        let mut fixture = Fixture::new();
        fixture.keep_spares(false);
        let (going, waiting, capped) = (fixture.create(), fixture.create(), fixture.create());
        fixture.arenas.set_cap(going, Some(1));
        let page = fixture.take(going).unwrap();
        let chunk = fixture.pages.memory.chunk_of(page.paddr());
        fixture.arenas.set_cap(capped, Some(1));
        let mut taken = fixture.take_chunks(capped, 1);
        fixture.arenas.set_total_cap(Some(fixture.arenas.given()));
        assert_eq!(fixture.arenas.starved(waiting), Some(Starved::Total));
        let free_chunks = fixture.arenas.free_chunk_count();

        // The page put back as the node frees it, its chunk going back, the lock not yet dropped.
        assert!(!fixture.arenas.put(&fixture.pages, page, false), "room before the chunk is back");
        assert_eq!(fixture.arenas.chunks(going), 1);
        assert_eq!(fixture.arenas.free_chunk_count(), free_chunks);
        assert_eq!(fixture.pages.memory.chunk_arena(chunk), Some(going));
        for arena in [going, waiting] {
            assert_eq!(fixture.arenas.starved(arena), None, "{arena:?}");
            assert_eq!(fixture.take(arena).err(), Some(Refused::GoingBack), "{arena:?}");
        }
        assert_eq!(fixture.arenas.starved(capped), Some(Starved::Arena));
        assert_eq!(fixture.take(capped).err(), Some(Starved::Arena.into()));

        assert!(fixture.return_chunks());
        assert_eq!(fixture.arenas.chunks(going), 0);
        assert_eq!(fixture.arenas.free_chunk_count(), free_chunks + 1);
        assert_eq!(fixture.pages.memory.chunk_arena(chunk), None);
        let page = fixture.take(waiting).expect("the chunk gone back");
        assert_eq!(fixture.pages.memory.chunk_of(page.paddr()), chunk);
        assert_eq!(fixture.arenas.starved(capped), Some(Starved::Arena));
        taken.push(page);
        fixture.arenas.set_total_cap(None);
        fixture.put_all(taken);
    }

    /// TASK-79.37: a chunk the host does not take back stays its arena's, its pages free again, as
    /// its spare, as it has none. No arena then takes a spare, which the host would refuse too,
    /// each allocation asking it in vain, until the host takes back, or maps, a chunk: the arena
    /// starting on that spare does without a new one; once its chunk has gone back, the next it
    /// takes comes with a spare.
    #[test]
    fn chunks_the_host_does_not_take_back_stay_their_arenas() {
        let mut fixture = Fixture::new();
        fixture.keep_spares(false);
        let arena = fixture.create();
        let taken = fixture.take_chunks(arena, 1);
        let chunk = fixture.pages.memory.chunk_of(taken[0].paddr());
        let free_chunks = fixture.arenas.free_chunk_count();
        for page in taken {
            assert!(!fixture.arenas.put(&fixture.pages, page, false));
        }
        fixture.refuse_chunks();
        assert_eq!(fixture.arenas.chunks(arena), 1);
        assert_eq!(fixture.pages.memory.chunk_arena(chunk), Some(arena));
        assert_eq!(fixture.arenas.free_chunk_count(), free_chunks);
        assert_eq!(fixture.arenas.entry(arena).spare, Some(chunk));
        assert_eq!(fixture.arenas.entry(arena).free.iter().count(), CHUNK_PAGES);

        fixture.arenas.keep_spares(&fixture.pages, true);
        let page = fixture.take(arena).unwrap();
        assert_eq!(fixture.pages.memory.chunk_of(page.paddr()), chunk);
        assert_eq!(fixture.arenas.chunks(arena), 1, "a spare asked of a host refusing chunks");
        assert!(fixture.put(page), "its chunk went back");
        assert_eq!(fixture.arenas.chunks(arena), 0);
        let page = fixture.take(arena).unwrap();
        assert_eq!(fixture.arenas.chunks(arena), 2, "a chunk, and a spare");
        fixture.put_all(vec![page]);
    }

    /// Relief is asked for each arena once a pass, at the cap last asked with, in order. A pass
    /// that frees nothing exhausts an arena starved still, until a page of it is freed, a cap
    /// changes, or a pass frees something; one not starved it leaves as it is. An arena gone has
    /// nothing to relieve.
    #[test]
    fn relief_is_asked_once_a_pass_and_exhausts_a_starved_arena() {
        let mut fixture = Fixture::new();
        let (first, second) = (fixture.create(), fixture.create());
        fixture.arenas.set_cap(first, Some(1));
        let mut taken = fixture.take_chunks(first, 1);
        assert_eq!(fixture.arenas.starved(first), Some(Starved::Arena));
        assert!(!fixture.arenas.ask_relief(second, Starved::Total));
        assert!(!fixture.arenas.ask_relief(first, Starved::Total));
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena));
        assert_eq!(
            fixture.arenas.take_asked(),
            [(first, Starved::Arena), (second, Starved::Total)]
        );
        assert!(fixture.arenas.take_asked().is_empty());

        fixture.arenas.relieved(first, 0);
        assert!(fixture.arenas.ask_relief(first, Starved::Arena), "exhausted");
        let _ = fixture.put(taken.pop().unwrap());
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena), "a page of it freed");
        assert_eq!(fixture.arenas.starved(first), None);
        fixture.arenas.relieved(first, 0);
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena), "not starved");

        taken.extend(fixture.take(first));
        fixture.arenas.relieved(first, 0);
        assert!(fixture.arenas.ask_relief(first, Starved::Arena), "exhausted again");
        fixture.arenas.relieved(first, page::SIZE as u64);
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena), "a pass freed a page");

        fixture.arenas.relieved(first, 0);
        fixture.arenas.set_cap(second, Some(4));
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena), "another's cap changed");
        fixture.arenas.relieved(first, 0);
        fixture.arenas.set_total_cap(None);
        assert!(!fixture.arenas.ask_relief(first, Starved::Arena), "the total cap changed");

        fixture.remove(second);
        assert!(fixture.arenas.ask_relief(second, Starved::Total), "gone");
        assert_eq!(fixture.arenas.take_asked(), [(first, Starved::Arena)]);
        fixture.put_all(taken);
    }

    /// TASK-108.1: pages taken together follow on from one another where those after are free, on
    /// the free list, and of the chunk, however the list orders them: freed in another order, as
    /// evictions free them, a run taken a page at a time from the list would be scattered. A page
    /// in use, or past the chunk, ends the run.
    #[test]
    fn pages_taken_after_another_follow_on_from_it() {
        let mut fixture = Fixture::new();
        let arena = fixture.create();
        let mut taken = fixture.take_chunks(arena, 1);
        let index = |page: VmPagePtr| page.paddr().0 / page::SIZE;
        let first = index(taken[0]);
        // Pages 8 to 11 of the chunk freed last first, so that the list holds them 11, 10, 9, 8,
        // behind page 8's: page 12 stays in use.
        let freed: Vec<VmPagePtr> = taken.drain(8..12).collect();
        for &page in &freed {
            let _ = fixture.put(page);
        }
        let head = fixture.take(arena).unwrap();
        assert_eq!(index(head), first + 11, "the list's head, freed last");
        assert_eq!(fixture.arenas.take_after(&fixture.pages, head), None, "page 12 is in use");
        let start = fixture.take(arena).unwrap();
        assert_eq!(index(start), first + 10);
        assert_eq!(fixture.arenas.take_after(&fixture.pages, start), None, "page 11 was taken");
        let _ = fixture.put(head);
        let _ = fixture.put(start);
        // Freed again: 10 at the head, then 11, 9, 8.
        let start = fixture.take(arena).unwrap();
        assert_eq!(index(start), first + 10);
        let next = fixture.arenas.take_after(&fixture.pages, start).unwrap();
        assert_eq!(index(next), first + 11);
        assert_eq!(fixture.arenas.take_after(&fixture.pages, next), None, "page 12 is in use");
        // The last page of the chunk has none after it of the chunk.
        let last = taken.pop().unwrap();
        assert_eq!(index(last), first + CHUNK_PAGES - 1);
        let _ = fixture.put(last);
        let last = fixture.take(arena).unwrap();
        assert_eq!(fixture.arenas.take_after(&fixture.pages, last), None, "past the chunk");
        taken.extend([start, next, last]);
        taken.extend(std::iter::from_fn(|| fixture.take(arena).ok()).take(2));
        fixture.put_all(taken);
    }

    /// TASK-106: a pass that frees nothing doesn't exhaust an arena starved while pages of it are
    /// quarantined, which make it room as they come back: what relief evicted that a window mapped,
    /// waiting for its VM process to unmap it, while the pass after finds nothing to evict. Its
    /// allocations wait for them rather than fail. Once none is to come back, it is exhausted.
    #[test]
    fn relief_exhausts_no_arena_with_pages_to_come_back() {
        let mut fixture = Fixture::new();
        let arena = fixture.create();
        fixture.arenas.set_cap(arena, Some(1));
        let mut taken = fixture.take_chunks(arena, 1);
        assert_eq!(fixture.arenas.starved(arena), Some(Starved::Arena));
        let evicted = taken.pop().unwrap();
        fixture.arenas.quarantine(&fixture.pages, evicted, true);
        fixture.arenas.relieved(arena, 0);
        assert!(!fixture.arenas.ask_relief(arena, Starved::Arena), "a page is to come back");

        fixture.arenas.quarantine(&fixture.pages, evicted, false);
        let _ = fixture.put(evicted);
        assert!(!fixture.arenas.ask_relief(arena, Starved::Arena), "it came back");
        taken.extend(fixture.take(arena));
        fixture.arenas.relieved(arena, 0);
        assert!(fixture.arenas.ask_relief(arena, Starved::Arena), "none is to come back");
        fixture.put_all(taken);
    }
}
