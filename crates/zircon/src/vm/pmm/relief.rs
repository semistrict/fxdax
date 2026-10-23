// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: the relief of starved arenas (`arenas`, TASK-50.3). An allocation that finds its
//! arena starved asks for relief, and one that can wait waits for it: the PMM's relief thread
//! frees pages for each arena asked for since its last pass, and starved still (TASK-79.76), with
//! the evictor the kernel's user gives it ([`relieve_with`]), the arena's own past its own cap, any
//! arena's past the total's or out of chunks; and then, with the kernel's own, on demand, those
//! the user's leaves, such as files', of the same: the arena's own
//! (`Evictor::evict_arena_on_demand`, TASK-79.76), or any arena's (`EvictionLevel::OnDemand`,
//! TASK-79.70). A pass that frees nothing for an arena starved still exhausts it: its allocations
//! fail rather than wait for ever, until a page of it is freed, a cap changes, or a later pass,
//! which each of them still asks for, frees something.
//!
//! Without relief, the kernel's evictor is asked to free a chunk's worth, of any arena's, on demand
//! (`EvictionLevel::OnDemand`), whatever their age, for each allocation that finds its arena
//! starved, or, memory never out, no free page at all (TASK-79.70), as nothing else makes room.

use super::arenas::Starved;
use super::{
    ALLOC_FLAG_CAN_WAIT, AllocFailure, AllocFailureType, NodeGuard, PmmNode, PmmNodeLocked, node,
};
use crate::kernel::deadline::Deadline;
use crate::kernel::event::AutounsignalEvent;
use crate::vm::evictor::{EvictionLevel, Output, TriggerReason};
use crate::vm::physmap::{ArenaId, CHUNK_SIZE};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use zx_status::Status;

/// What a pass frees for an arena at least: a chunk's worth twice over, so that the next
/// allocations find room.
const RELIEF_BYTES: u64 = 2 * CHUNK_SIZE as u64;

/// The relief thread's: what wakes it, and whether it runs.
struct Reliever {
    /// Signaled as an allocation asks for relief.
    wanted: AutounsignalEvent,
    /// Whether the thread runs: once it has gone, as its evictor panicked, the allocations it
    /// would make room for fail.
    running: AtomicBool,
}

/// The relief thread's, if [`relieve_with`] started it.
static RELIEVER: OnceLock<Option<Arc<Reliever>>> = OnceLock::new();

fn reliever() -> Option<&'static Reliever> {
    RELIEVER.get()?.as_deref()
}

/// Whether starved arenas are relieved: the relief thread started, whether or not it runs still.
pub(super) fn started() -> bool {
    reliever().is_some()
}

/// fxdax: starts the relief of starved arenas, on a thread of the PMM's own, which frees pages
/// with `evict`: given the arena whose pages to free, or none for any arena's, and how many bytes
/// at least, it frees what it may of them and returns the bytes it freed. It is called without the
/// PMM's lock, and must not wait for an allocation that waits for relief, as its own thread
/// relieves them. What it leaves, the kernel's evictor frees on demand, of the same arenas.
///
/// Relief starts once: a later call fails with `AlreadyExists`, whether or not the first started
/// its thread. Under Shuttle no thread starts, as none of the kernel's does (`crate::sync`), and
/// the kernel's evictor frees what it may.
pub fn relieve_with(
    evict: impl FnMut(Option<ArenaId>, u64) -> u64 + Send + 'static,
) -> std::io::Result<()> {
    let mut started = Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "the arenas' relief has started",
    ));
    RELIEVER.get_or_init(|| {
        let reliever = Arc::new(Reliever {
            wanted: AutounsignalEvent::new(false),
            running: AtomicBool::new(true),
        });
        let thread = reliever.clone();
        let spawned =
            crate::sync::try_spawn("arena-relief-thread", move || relieve(&thread, evict));
        let installed = spawned.as_ref().is_ok_and(Option::is_some).then_some(reliever);
        started = spawned.map(drop);
        installed
    });
    started
}

/// The relief thread: frees pages with `evict` for each arena asked for since its last pass, and
/// starved still at the cap it was asked at, without the PMM's lock, the arena's own at its own
/// cap, any arena's at the total's, those `evict` leaves with the kernel's evictor, and records what
/// it freed, which the arena's allocations waiting then try again with.
fn relieve(reliever: &Reliever, mut evict: impl FnMut(Option<ArenaId>, u64) -> u64) {
    let _gone = Gone(reliever);
    loop {
        let status = reliever.wanted.wait(&Deadline::infinite());
        debug_assert!(status.is_ok());
        // An allocation of the node's asked: it has booted.
        let node = node();
        let asked = node.locked().arenas.take_asked();
        for (arena, starved) in asked {
            // An ask made stale, by a pass that relieved the arena since, as its allocations asked
            // again while it ran (TASK-79.76), or by a cap changed, has nothing to relieve: a pass
            // would evict as much again for no allocation. Those waiting were woken as the arena
            // changed, and ask again if they must.
            if node.locked().arenas.starved(arena) != Some(starved) {
                continue;
            }
            let from = match starved {
                Starved::Arena => Some(arena),
                Starved::Total => None,
                Starved::Removed => unreachable!("relief asked for {arena:?}, removed"),
            };
            let mut freed = evict(from, RELIEF_BYTES);
            if freed < RELIEF_BYTES {
                let rest = RELIEF_BYTES - freed;
                let evicted = match from {
                    // Its own pages alone make it room, and no other arena's are evicted for it.
                    Some(arena) => node.evictor.evict_arena_on_demand(arena, rest),
                    None => {
                        let (level, reason) = (EvictionLevel::OnDemand, TriggerReason::Other);
                        let evicted =
                            node.evictor.evict_synchronous(rest, 0, level, Output::NoPrint, reason);
                        evicted.counts
                    }
                };
                freed += evicted.non_loaned_total() * page::SIZE as u64;
            }
            node.locked().arenas.relieved(arena, freed);
            node.room.pulse();
        }
    }
}

/// Marks relief gone as its thread ends, as its evictor panicked: the allocations it would make
/// room for fail, those waiting for it now too.
struct Gone<'a>(&'a Reliever);

impl Drop for Gone<'_> {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
        node().room.pulse();
    }
}

impl PmmNode {
    /// fxdax: what an allocation of `size` pages of `arena`'s fails with, which found it starved,
    /// held to `starved`, as the node's lock, `locked`, shows it still. With relief, it asks for
    /// it, and one that can wait waits for it (`wait_for_single_page_allocation_in`), unless the
    /// arena is exhausted, or relief gone, when it fails. Without, eviction is asked to free a
    /// chunk's worth, which one that can wait waits for. Both are asked for without the lock. A
    /// failure with no free page at all is the node's (`starved_failure_locked`). One of an arena
    /// removed, which nothing makes room, fails, asking for neither.
    pub(super) fn starved_status(
        &self,
        mut locked: NodeGuard<'_>,
        arena: ArenaId,
        starved: Starved,
        alloc_flags: u32,
        size: usize,
    ) -> Status {
        if starved == Starved::Removed {
            return Status::NO_MEMORY;
        }
        let can_wait = alloc_flags & ALLOC_FLAG_CAN_WAIT != 0;
        let Some(reliever) = reliever() else {
            let status = if can_wait {
                Status::SHOULD_WAIT
            } else {
                self.starved_failure_locked(&mut locked, size)
            };
            drop(locked);
            self.evictor.evict_asynchronous(
                CHUNK_SIZE as u64,
                0,
                EvictionLevel::OnDemand,
                Output::NoPrint,
            );
            return status;
        };
        if !reliever.running.load(Ordering::Acquire) {
            return self.starved_failure_locked(&mut locked, size);
        }
        let fails = locked.arenas.ask_relief(arena, starved) || !can_wait;
        let status = if fails {
            self.starved_failure_locked(&mut locked, size)
        } else {
            Status::SHOULD_WAIT
        };
        drop(locked);
        reliever.wanted.signal();
        status
    }

    /// fxdax: fails an allocation of `size` pages that found its arena starved, under the node's
    /// lock, `locked`: with `NO_MEMORY`, reported as the node's failure if there is no free page at
    /// all, of any arena's, as memory is out, as upstream's is (TASK-79.70); otherwise the
    /// arena's own, as other arenas' chunks hold the free pages.
    fn starved_failure_locked(&self, locked: &mut PmmNodeLocked, size: usize) -> Status {
        let free_count = self.free_count.load(Ordering::Relaxed);
        if free_count == 0 {
            let failure = AllocFailure { r#type: AllocFailureType::Pmm, size, free_count };
            self.report_alloc_failure_locked(locked, failure);
        }
        Status::NO_MEMORY
    }
}
