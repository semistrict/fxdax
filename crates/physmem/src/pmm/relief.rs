// Copyright 2026 The fxdax Authors.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! fxdax's own: the relief of starved arenas (`arenas`, TASK-50.3). An allocation that finds its
//! arena starved asks for relief, and one that can wait waits for it: the PMM's relief thread
//! frees pages for each arena asked for since its last pass, and starved still (TASK-79.76), with
//! the evictor its user gives it ([`relieve_with`]), the arena's own past its own cap, any
//! arena's past the total's or out of chunks. A pass that frees nothing for an arena starved still
//! exhausts it: its allocations fail rather than wait for ever, until a page of it is freed, a cap
//! changes, or a later pass, which each of them still asks for, frees something.
//!
//! Without relief, nothing makes a starved arena room: its allocations fail, whether or not they
//! may wait.

use super::arenas::Starved;
use super::{Failed, NodeGuard, PmmNode, node};
use crate::Wait;
use crate::physmap::{ArenaId, CHUNK_SIZE};
use crate::sync::Signal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// What a pass frees for an arena at least: a chunk's worth twice over, so that the next
/// allocations find room. What it asks of [`relieve_with`]'s evictor.
pub const RELIEF_BYTES: u64 = 2 * CHUNK_SIZE as u64;

/// The relief thread's: what wakes it, and whether it runs.
struct Reliever {
    /// Signaled as an allocation asks for relief.
    wanted: Signal,
    /// Whether the thread runs: once it has gone, as its evictor panicked, the allocations it
    /// would make room for fail.
    running: AtomicBool,
}

/// The relief thread's, if [`relieve_with`] started it.
static RELIEVER: OnceLock<Option<Arc<Reliever>>> = OnceLock::new();

fn reliever() -> Option<&'static Reliever> {
    RELIEVER.get()?.as_deref()
}

/// fxdax: starts the relief of starved arenas, on a thread of the PMM's own, which frees pages
/// with `evict`: given the arena whose pages to free, or none for any arena's, and how many bytes
/// at least, it frees what it may of them and returns the bytes it freed. It is called without the
/// PMM's lock, and must not wait for an allocation that waits for relief, as its own thread
/// relieves them.
///
/// Relief starts once: a later call fails with `AlreadyExists`, whether or not the first started
/// its thread. Under Shuttle no thread starts (`crate::sync`), and starved arenas' allocations
/// fail, as without relief.
pub fn relieve_with(
    evict: impl FnMut(Option<ArenaId>, u64) -> u64 + Send + 'static,
) -> std::io::Result<()> {
    let mut started = Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "the arenas' relief has started",
    ));
    RELIEVER.get_or_init(|| {
        let reliever = Arc::new(Reliever { wanted: Signal::new(), running: AtomicBool::new(true) });
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
/// cap, any arena's at the total's, and records what it freed, which the arena's allocations
/// waiting then try again with.
fn relieve(reliever: &Reliever, mut evict: impl FnMut(Option<ArenaId>, u64) -> u64) {
    let _gone = Gone(reliever);
    loop {
        reliever.wanted.wait();
        let node = node();
        let asked = node.locked().take_asked();
        for (arena, starved) in asked {
            // An ask made stale, by a pass that relieved the arena since, as its allocations asked
            // again while it ran (TASK-79.76), or by a cap changed, has nothing to relieve: a pass
            // would evict as much again for no allocation. Those waiting were woken as the arena
            // changed, and ask again if they must.
            if node.locked().starved(arena) != Some(starved) {
                continue;
            }
            let from = match starved {
                Starved::Arena => Some(arena),
                Starved::Total => None,
                Starved::Removed => unreachable!("relief asked for {arena:?}, removed"),
            };
            let freed = evict(from, RELIEF_BYTES);
            node.locked().relieved(arena, freed);
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
    /// fxdax: what an allocation of `arena`'s fails with, which found it starved, held to
    /// `starved`, as the node's lock, `locked`, shows it still. With relief, it asks for it, and
    /// one that may `wait` is told to wait for it, unless the arena is exhausted, or relief gone,
    /// when it fails. Relief is asked for without the lock. Without relief it fails. A failure with
    /// no free page at all is the node's (`starved_failure`). One of an arena removed, which
    /// nothing makes room, fails, asking for nothing.
    pub(super) fn starved_status(
        &self,
        mut locked: NodeGuard<'_>,
        arena: ArenaId,
        starved: Starved,
        wait: Wait,
    ) -> Failed {
        if starved == Starved::Removed {
            return Failed::NoMemory;
        }
        let Some(reliever) = reliever() else { return self.starved_failure() };
        if !reliever.running.load(Ordering::Acquire) {
            return self.starved_failure();
        }
        let fails = locked.ask_relief(arena, starved) || wait == Wait::No;
        let status = if fails { self.starved_failure() } else { Failed::ShouldWait };
        drop(locked);
        reliever.wanted.signal();
        status
    }

    /// fxdax: fails an allocation that found its arena starved: reported as the node's failure if
    /// there is no free page at all, of any arena's, as memory is out (TASK-79.70); otherwise the
    /// arena's own, as other arenas' chunks hold the free pages.
    fn starved_failure(&self) -> Failed {
        if self.count_free_pages() == 0 {
            self.report_alloc_failure();
        }
        Failed::NoMemory
    }
}
