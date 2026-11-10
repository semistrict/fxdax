// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A store crashes at random points of random workloads, and recovers to a prefix of its commits:
//! exactly the extents and root of an epoch at least as late as the last commit made durable, by
//! a durable commit or a flush of commits only ordered; fxdax: as its open segment closes between
//! commits too, as an uploader has it close.
//!
//! fxdax: a round is a function of its seed (TASK-79.79): its store does its I/O on the test's
//! thread alone, none on threads of its own ([`MemDir::serial`]), so that the operation its file
//! system fails from, and what the crash keeps, are the same each time it is run. The Shuttle tests
//! run the same rounds ([`rounds`]) with the store's threads, which frame the segments commits
//! close, and sync a flush's files at once, while the workload goes on: on Shuttle's schedules,
//! seeded by the rounds, so that each is still a function of its seed (tests/shuttle/crash.rs).

mod kept;
mod rounds;

use extent_store::MemDir;
use kept::{CUT_ROUNDS, KEPT_ROUNDS, cut, kept};
use rounds::{ROUNDS, recovering, same, twice};

/// fxdax: each round runs twice, at once, and its two runs' crashes leave the same directories,
/// and they end with the same: a round is a function of its seed alone, so that a round that failed
/// fails again, and a round that passed says what it seems (TASK-79.79).
#[test]
fn recovers_a_prefix_of_its_commits() {
    let mut crashes_mid_commit = 0;
    for round in 0..ROUNDS {
        let ((first, mid_commit), (second, _)) = twice(|| recovering(round, MemDir::serial()));
        same(round, &first, &second);
        crashes_mid_commit += mid_commit as u64;
    }
    // The crashes hit commits as well as what came between them.
    assert!(crashes_mid_commit > ROUNDS / 10, "{crashes_mid_commit} crashes in commits");
}

/// fxdax: a store keeping its branch in a bucket, crashing as its uploader uploads, recovers from
/// the bucket alone at least what its uploader acknowledged, and from its disk at least what it
/// made durable (TASK-165.9's AC #4); the host's collector passing over the bucket all along, cut
/// in a pass as it crashes, or before, and a full pass after (TASK-291.2).
#[test]
fn recovers_from_its_bucket_with_and_without_its_disk() {
    let (mut uploaded, mut ahead, mut cut, mut compacted, mut deleted) = (0, 0, 0, 0, 0);
    for round in 0..KEPT_ROUNDS {
        let found = kept(round);
        uploaded += found.acknowledged as u64;
        ahead += found.ahead as u64;
        cut += found.cut as u64;
        compacted += found.compacted as u64;
        deleted += found.deleted as u64;
    }
    // The crashes hit uploads under way, and stores whose disks held more than the bucket; the
    // collector's cuts, passes under way; and its passes compacted and deleted what the stores
    // had uploaded.
    assert!(uploaded > KEPT_ROUNDS / 4, "{uploaded} rounds acknowledged a commit");
    assert!(ahead > KEPT_ROUNDS / 10, "{ahead} rounds held commits the bucket lacked");
    assert!(cut > KEPT_ROUNDS / 4, "{cut} rounds cut a pass before the crash");
    assert!(compacted > KEPT_ROUNDS / 10, "{compacted} rounds' passes compacted");
    assert!(deleted > KEPT_ROUNDS / 3, "{deleted} rounds' passes deleted");
}

/// fxdax: a store's opening cut by a crash at each of its writes in turn leaves what opens again
/// to at least what the store made durable (TASK-165.9).
#[test]
fn openings_cut_at_each_write_open_again() {
    let writes: u64 = (0..CUT_ROUNDS).map(cut).sum();
    // The openings write: a segment closed again, the WAL's base, a restated capacity.
    assert!(writes > CUT_ROUNDS, "{writes} writes in {CUT_ROUNDS} openings");
}
