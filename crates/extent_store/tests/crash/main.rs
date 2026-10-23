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

mod rounds;

use extent_store::MemDir;
use rounds::{FORK_ROUNDS, ROUNDS, forking, recovering, same, twice};

/// TASK-79.79: a round is a function of its seed alone: run twice, its crashes leave the same
/// directories, and it ends with the same, so that a round that failed fails again, and a round
/// that passed says what it seems.
#[test]
fn crashes_are_functions_of_their_rounds() {
    for round in 0..ROUNDS {
        let (first, second) = twice(|| recovering(round, MemDir::serial()).0);
        same(round, &first, &second);
    }
}

#[test]
fn recovers_a_prefix_of_its_commits() {
    let crashes_mid_commit =
        (0..ROUNDS).filter(|&round| recovering(round, MemDir::serial()).1).count() as u64;
    // The crashes hit commits as well as what came between them.
    assert!(crashes_mid_commit > ROUNDS / 10, "{crashes_mid_commit} crashes in commits");
}

/// A parent and its fork, which inherited its extents, run workloads by turns and crash, and each
/// recovers a prefix of its own commits, whatever the other freed; or the fork is opened only
/// after the crash, and holds what its parent held at the fork, whatever its parent freed and
/// cleaned since.
#[test]
fn forks_recover_prefixes_of_their_commits() {
    for round in 0..FORK_ROUNDS {
        forking(round, MemDir::serial());
    }
}

/// TASK-79.79: so is a round of [`forks_recover_prefixes_of_their_commits`].
#[test]
fn forks_crashes_are_functions_of_their_rounds() {
    for round in 0..FORK_ROUNDS {
        let (first, second) = twice(|| forking(round, MemDir::serial()));
        same(round, &first, &second);
    }
}
