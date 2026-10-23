// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The store's locks and condition variables: std's, or with the `shuttle` feature Shuttle's
//! (awslabs/shuttle), whose scheduler then chooses every interleaving of the threads using a store,
//! and replays a failing one from its seed (tests/shuttle/, TASK-44.4). Arcs are std's either
//! way.
//!
//! And the rules the store's concurrency relies on, which those tests turn off one at a time to
//! see a test fail: a test that never fails proves nothing.

#[cfg(feature = "shuttle")]
pub(crate) use shuttle::sync::{Condvar, Mutex, MutexGuard};
#[cfg(feature = "shuttle")]
pub(crate) use shuttle::thread;
#[cfg(not(feature = "shuttle"))]
pub(crate) use std::sync::{Condvar, Mutex, MutexGuard};
#[cfg(not(feature = "shuttle"))]
pub(crate) use std::thread;

/// The rules the store's concurrency relies on, each of which its Shuttle tests show to be needed
/// by turning it off.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// A free keeps the extent's data for the uploader until it releases the commit that freed it.
    pub keep_frees: bool,
    /// Closing the open segment between commits waits for the writes in it to finish.
    pub close_waits_for_writers: bool,
    /// A commit waits for the writes in progress to finish before it seals the open extents.
    pub commit_waits_for_writes: bool,
    /// A commit, or a closing between commits, that waited for the writes in progress goes on
    /// only if the store did not fail, nor the branch close, meanwhile (TASK-83).
    pub recheck_after_waiting: bool,
    /// A write to an extent freed since it began is dropped as it finishes (TASK-79.1).
    pub drop_writes_to_freed_extents: bool,
    /// A commit waits for cleaning's copies in progress, as for writes, so that the segment it
    /// closes takes none after it (TASK-79.2).
    pub commits_wait_for_copies: bool,
    /// A copy of cleaning's becomes its extent's place, at the next commit, only if it is in the
    /// open segment, whose footer names it then (TASK-79.2).
    pub moves_into_the_open_segment: bool,
    /// The uploader is given commits and closed segments only once they are durable.
    pub durable_only: bool,
    /// A commit is durable once its flush has synced it, not once a flush has taken it.
    pub durable_once_synced: bool,
    /// Closing a branch waits for a flush in flight to finish.
    pub close_waits_for_flushes: bool,
    /// Closing a branch waits for the segments closed between commits to be framed (TASK-41.1).
    pub close_waits_for_framings: bool,
    /// A fork waits for the segments closed between commits to be framed, which its child reads
    /// from their framed files (TASK-41.1).
    pub forks_wait_for_framings: bool,
    /// A free of an extent sealed before keeps its data while a view older than the free lives.
    pub keep_frees_for_views: bool,
    /// A free of an extent a view pins keeps its data while it does.
    pub keep_pins: bool,
    /// What a commit's frees keep is decided as the commit is published to views, at once.
    pub decide_at_publish: bool,
    /// A view refuses extents sealed after its commit.
    pub views_refuse_later_extents: bool,
    /// A view reads, of the frees kept, only those after its commit, and those it pins.
    pub views_hold_their_commit: bool,
    /// Openings, forks and deletions of a branch being forked or deleted wait until that is done,
    /// rather than fail, and lists of the branches wait for every fork and deletion in flight.
    pub wait_for_forks_and_deletions: bool,
}

impl Rules {
    pub const ALL: Self = Self {
        keep_frees: true,
        close_waits_for_writers: true,
        commit_waits_for_writes: true,
        recheck_after_waiting: true,
        drop_writes_to_freed_extents: true,
        commits_wait_for_copies: true,
        moves_into_the_open_segment: true,
        durable_only: true,
        durable_once_synced: true,
        close_waits_for_flushes: true,
        close_waits_for_framings: true,
        forks_wait_for_framings: true,
        keep_frees_for_views: true,
        keep_pins: true,
        decide_at_publish: true,
        views_refuse_later_extents: true,
        views_hold_their_commit: true,
        wait_for_forks_and_deletions: true,
    };
}

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(not(feature = "shuttle"))]
pub(crate) fn rules() -> Rules {
    Rules::ALL
}

#[cfg(feature = "shuttle")]
static RULES: std::sync::Mutex<Rules> = std::sync::Mutex::new(Rules::ALL);

/// The rules in force: all of them, but in a Shuttle test that turned one off.
#[cfg(feature = "shuttle")]
pub fn rules() -> Rules {
    *RULES.lock().unwrap()
}

/// Sets the rules in force, for the Shuttle tests that turn one off.
#[cfg(feature = "shuttle")]
pub fn set_rules(rules: Rules) {
    *RULES.lock().unwrap() = rules;
}
