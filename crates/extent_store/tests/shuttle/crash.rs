// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The crash tests' rounds (tests/crash/), with the store's threads (TASK-79.79): in a directory
//! whose store frames the segments its commits close on threads of its own, and syncs a flush's
//! files at once, as on the host, while the workload goes on, so that the file system fails, and
//! the crash comes, where their I/O and the workload's interleave: a framing's write failing after
//! a later commit's record was written, say, which a serial directory never sees. Each round runs
//! under Shuttle, on a random schedule seeded by the round, as its workload is: so that it is a
//! function of its seed still, which each round, run twice at once, shows by leaving the same
//! directories, and a round that failed fails again.
//!
//! So a failing round is replayed by its number, which its failure names, not by the schedule
//! Shuttle prints: [`same`] compares a round's two runs once both are done, outside any of
//! Shuttle's executions, and of its failure Shuttle's panic hook prints the schedule of the last
//! execution on the test's thread, which replaying would pass.

#[path = "../crash/rounds.rs"]
mod rounds;

use super::config;
use extent_store::{MemDir, Rules, set_rules};
use rounds::{FORK_ROUNDS, ROUNDS, forking, recovering, same, twice};
use shuttle::Runner;
use shuttle::scheduler::RandomScheduler;
use std::sync::{Arc, Mutex};

/// Round `round` of `run`, in a directory whose store does I/O on threads of its own, under
/// Shuttle, on a random schedule seeded by the round: what it returned.
fn concurrently<T: Send + 'static>(round: u64, run: fn(u64, MemDir) -> T) -> T {
    // std's: the result leaves the run in it.
    let returned = Arc::new(Mutex::new(None));
    let theirs = returned.clone();
    let scheduler = RandomScheduler::new_from_seed(round, 1);
    Runner::new(scheduler, config()).run(move || {
        *theirs.lock().unwrap() = Some(run(round, MemDir::new()));
    });
    returned.lock().unwrap().take().expect("the round ran")
}

#[test]
fn crashes_recover_prefixes_of_their_commits_concurrently() {
    set_rules(Rules::ALL);
    let (mut crashes_mid_commit, mut crashes_elsewhere) = (0, 0);
    for round in 0..ROUNDS {
        let ((first, mid_commit, elsewhere), (second, _, _)) = twice(|| {
            concurrently(round, |round, dir| {
                let (left, mid_commit) = recovering(round, dir.clone());
                (left, mid_commit, dir.failed_elsewhere())
            })
        });
        same(round, &first, &second);
        crashes_mid_commit += mid_commit as u64;
        crashes_elsewhere += elsewhere as u64;
    }
    // The crashes hit commits as well as what came between them; and the store's own threads'
    // I/O, a framing's or a flush's syncs, as well as the workload's.
    assert!(crashes_mid_commit > ROUNDS / 10, "{crashes_mid_commit} crashes in commits");
    assert!(crashes_elsewhere > ROUNDS / 30, "{crashes_elsewhere} crashes in the store's threads");
}

#[test]
fn forks_recover_prefixes_of_their_commits_concurrently() {
    set_rules(Rules::ALL);
    let mut crashes_elsewhere = 0;
    for round in 0..FORK_ROUNDS {
        let ((first, elsewhere), (second, _)) = twice(|| {
            concurrently(round, |round, dir| (forking(round, dir.clone()), dir.failed_elsewhere()))
        });
        same(round, &first, &second);
        crashes_elsewhere += elsewhere as u64;
    }
    // The crashes hit the store's own threads' I/O, as well as the workloads'.
    assert!(
        crashes_elsewhere > FORK_ROUNDS / 30,
        "{crashes_elsewhere} crashes in the store's threads"
    );
}
