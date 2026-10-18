// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A deterministic simulation of branches in object storage (TASK-28.1): doc-7's protocol run by
//! writers, forks, takeovers, deletions and collectors over a bucket that delays, reorders and
//! fails requests, checked after every step against what each branch's guests did and were told,
//! and, once everything stops, for garbage left behind.
//!
//! Each case doc-7 lists is a profile that makes it likely, run over many seeds; each rule of the
//! protocol has a test that turns it off and expects some seed to find the violation it prevents.

pub(crate) mod bucket;
mod world;

use bucket::Direct;

use crate::{Rules, set_rules};
use std::ops::Range;
use world::{Profile, Weights, World};

/// The violations a run of `seed` finds, and, if any, what the bucket held.
fn violations(seed: u64, profile: Profile) -> (Vec<String>, String) {
    let mut world = World::new(seed, profile);
    world.run();
    let description = if world.violations.is_empty() { String::new() } else { world.describe() };
    (world.violations, description)
}

/// Runs `seeds`, failing at the first that finds a violation. `SIM_SOAK` multiplies how many.
fn holds(profile: Profile, seeds: Range<u64>) {
    let soak: u64 = std::env::var("SIM_SOAK").map_or(1, |soak| soak.parse().unwrap());
    for seed in seeds.start..seeds.start + (seeds.end - seeds.start) * soak {
        let (found, description) = violations(seed, profile);
        assert!(found.is_empty(), "seed {seed}: {found:#?}\n{description}");
    }
}

/// Runs `seeds` with `rules`, expecting one of them to find a violation other than garbage left:
/// the rules keep branches whole; leaving garbage is a lesser failure. It stops at the first, so
/// a wide range costs time only when no seed finds one. Changing the simulation moves the seeds
/// that do; a test that stops finding one needs a profile likelier to make its case.
fn needed(rules: Rules, profile: Profile, seeds: Range<u64>) {
    set_rules(rules);
    let found = seeds.clone().find_map(|seed| {
        let (found, _) = violations(seed, profile);
        let unsafe_ = found.iter().any(|violation| !violation.starts_with("garbage left"));
        unsafe_.then_some((seed, found))
    });
    set_rules(Rules::ALL);
    let (seed, found) = found.unwrap_or_else(|| panic!("no seed of {seeds:?} needed the rule"));
    println!("seed {seed}: {found:#?}");
}

/// A test that 200 seeds of `$profile` find no violation.
macro_rules! test_holds {
    ($name:ident, $profile:expr) => {
        #[test]
        fn $name() {
            holds($profile, 0..200);
        }
    };
}

/// A test that `$rule` is needed: that one of 20,000 seeds of `$profile` without it finds a
/// violation.
macro_rules! test_needs {
    ($name:ident, $rule:ident, $profile:expr) => {
        #[test]
        fn $name() {
            needed(Rules { $rule: false, ..Rules::ALL }, $profile, 0..20_000);
        }
    };
}

fn weights(change: impl FnOnce(&mut Weights)) -> Profile {
    let mut profile = Profile::default();
    change(&mut profile.weights);
    profile
}

/// `SIM_SEEDS` seeds (1 by default) from `SIM_SEED` of one profile, as `SIM_PROFILE` says, with
/// the rules `SIM_OFF` names (separated by commas) off, to debug them.
#[test]
#[ignore]
fn one() {
    let seed = std::env::var("SIM_SEED").map_or(0, |seed| seed.parse().unwrap());
    let mut rules = Rules::ALL;
    for name in
        std::env::var("SIM_OFF").unwrap_or_default().split(',').filter(|name| !name.is_empty())
    {
        match name {
            "acks_after_lease" => rules.acks_after_lease = false,
            "fence" => rules.fence = false,
            "continuity" => rules.continuity = false,
            "epoch_sequences" => rules.epoch_sequences = false,
            "records_after_ack" => rules.records_after_ack = false,
            "candidates_first" => rules.candidates_first = false,
            "newest_checkpoint" => rules.newest_checkpoint = false,
            "forks_from_states" => rules.forks_from_states = false,
            "orphans_after_successor" => rules.orphans_after_successor = false,
            "tokens" => rules.tokens = false,
            other => panic!("no rule {other}"),
        }
    }
    set_rules(rules);
    let profile = match std::env::var("SIM_PROFILE").as_deref().unwrap_or("everything") {
        "everything" => Profile::default(),
        "forks" => forks(),
        "bases" => deleted_bases(),
        "stale" => stale_placements(),
        "collected" => collected(),
        "takeovers" => takeovers(),
        "crashes" => crashes(),
        "collectors" => collectors(),
        "taken" => taken(),
        "children" => children(),
        "recovering" => recovering(),
        other => panic!("no profile {other}"),
    };
    let count = std::env::var("SIM_SEEDS").map_or(1, |count| count.parse().unwrap());
    holds(profile, seed..seed + count);
}

/// Everything, at the default rates.
#[test]
fn everything() {
    holds(Profile::default(), 0..300);
}

/// Case 1: frees before and after forks, by parents and children. Case 2: chains of generations,
/// some deleted in the middle.
fn forks() -> Profile {
    weights(|weights| {
        weights.fork = 4;
        weights.open = 6;
        weights.delete = 2;
    })
}

test_holds!(forks_and_frees, forks());

/// Bases deleted while their forks live (TASK-39.4): the forks write, fork and are taken over, and
/// collectors run, keeping what the forks need of the bases.
fn deleted_bases() -> Profile {
    weights(|weights| {
        weights.fork = 4;
        weights.open = 6;
        weights.delete_base = 3;
        weights.collect = 6;
        weights.checkpoint = 4;
        weights.takeover = 2;
    })
}

test_holds!(bases_deleted_under_live_forks, deleted_bases());

/// Placements cut off taking branches back, with their earlier fencing tokens, from the ones after
/// them, while those write and are taken over in turn (TASK-39.7).
fn stale_placements() -> Profile {
    weights(|weights| {
        weights.takeover = 4;
        weights.open = 6;
        weights.reclaim = 4;
        weights.close = 4;
        weights.crash = 2;
    })
}

test_holds!(stale_placements_take_nothing_back, stale_placements());

test_needs!(fencing_needs_tokens, tokens, stale_placements());

/// Case 3: forks racing collection and compaction.
fn collected() -> Profile {
    let mut profile = weights(|weights| {
        weights.fork = 3;
        weights.open = 6;
        weights.collect = 8;
        weights.checkpoint = 6;
        weights.tick = 6;
    });
    profile.options.compact_below = 0.9;
    profile
}

test_holds!(forks_racing_collection, collected());

/// Forks whose children open soon after, while collectors run: a child's record and lease come
/// while a pass reads branches.
fn children() -> Profile {
    weights(|weights| {
        weights.fork = 6;
        weights.open = 10;
        weights.collect = 10;
        weights.crash = 2;
    })
}

test_holds!(forks_with_children_opening, children());

test_needs!(collection_needs_candidates_first, candidates_first, collected());

test_needs!(collection_needs_forks_from_states, forks_from_states, collected());

/// Case 4: takeovers while the writer before is still uploading. Case 9: a stale writer
/// recreating what the collector deleted.
fn takeovers() -> Profile {
    let mut profile = weights(|weights| {
        weights.takeover = 6;
        weights.open = 6;
        weights.close = 10;
        weights.collect = 4;
        weights.checkpoint = 4;
    });
    profile.fail_after = 0.15;
    profile
}

test_holds!(takeovers_while_uploading, takeovers());

test_needs!(takeovers_need_acks_after_lease, acks_after_lease, Profile::default());

test_needs!(takeovers_need_fences, fence, Profile::default());

test_needs!(takeovers_need_continuity, continuity, takeovers());

/// Takeovers while collectors run: an extent of the writer taken over, uploaded, its segment not
/// yet.
fn taken() -> Profile {
    let mut profile = weights(|weights| {
        weights.takeover = 8;
        weights.open = 8;
        weights.close = 4;
        weights.collect = 10;
        weights.tick = 4;
    });
    profile.fail_after = 0.1;
    profile
}

test_holds!(takeovers_racing_collection, taken());

test_needs!(takeovers_need_orphans_after_successors, orphans_after_successor, taken());

// Case 10: extents minted after a takeover.
test_needs!(takeovers_need_epoch_sequences, epoch_sequences, takeovers());

/// Case 5: crashes between a segment's upload and the next, and between a fork and its child's
/// record. Case 6: frees in commits never made durable.
fn crashes() -> Profile {
    let mut profile = weights(|weights| {
        weights.crash = 4;
        weights.fork = 3;
        weights.open = 8;
    });
    profile.fail_before = 0.1;
    profile
}

test_holds!(crashes_anywhere, crashes());

test_needs!(forks_need_records_after_acks, records_after_ack, crashes());

/// Case 7: collectors stopping midway, two at once, one fenced by another. Case 8: writers' indexes
/// older than compactions.
fn collectors() -> Profile {
    let mut profile = weights(|weights| {
        weights.collect = 10;
        weights.stop_collector = 4;
        weights.checkpoint = 4;
        weights.tick = 8;
    });
    profile.options.compact_below = 0.9;
    profile
}

test_holds!(collectors_stopping_and_racing, collectors());

/// Writers taking branches over, recovering them, while checkpoints come and collectors delete
/// what they cover.
fn recovering() -> Profile {
    let mut profile = weights(|weights| {
        weights.commit = 20;
        weights.close = 15;
        weights.checkpoint = 3;
        weights.takeover = 8;
        weights.open = 8;
        weights.collect = 10;
        weights.tick = 6;
    });
    profile.steps = 800;
    profile.options.compact_below = 0.9;
    profile.options.grace = 1;
    profile
}

test_holds!(recoveries_racing_collection, recovering());

test_needs!(recoveries_need_newest_checkpoints, newest_checkpoint, takeovers());

/// A create that lands and then fails, retried, finds its own object and succeeds; one that finds
/// another's does not.
#[test]
fn ambiguous_creates_are_resolved_by_reading_back() {
    use crate::bucket::{Created, create};
    use bucket::{Fault, SimBucket};
    use futures::executor::LocalPool;
    use futures::task::LocalSpawnExt as _;
    use std::cell::RefCell;
    use std::rc::Rc;

    let sim = SimBucket::new();
    let mut pool = LocalPool::new();
    let results = Rc::new(RefCell::new(Vec::new()));
    for (key, bytes) in [("ours", b"ours"), ("theirs", b"ours")] {
        let (sim, results) = (sim.clone(), results.clone());
        pool.spawner()
            .spawn_local(async move {
                let created = create(&*sim, key, bytes.to_vec()).await;
                results.borrow_mut().push((key, matches!(created, Created::Created)));
            })
            .unwrap();
    }
    pool.run_until_stalled();
    // "theirs" holds another's object before its create lands.
    futures::executor::block_on(crate::bucket::Bucket::put(
        &Direct(sim.clone()),
        "theirs",
        b"theirs".to_vec(),
        crate::bucket::PutMode::Create,
    ))
    .unwrap();
    // The first create of each lands and fails; what follows succeeds.
    let mut first = true;
    while sim.waiting() > 0 {
        sim.complete(0, if first { Fault::After } else { Fault::None });
        first = false;
        pool.run_until_stalled();
    }
    let mut results = results.borrow().clone();
    results.sort();
    assert_eq!(results, [("ours", true), ("theirs", false)]);
}
