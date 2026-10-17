//! A deterministic simulation of branches in object storage (TASK-28.1): doc-7's protocol run by
//! writers, forks, takeovers, deletions and collectors over a bucket that delays, reorders and
//! fails requests, checked after every step against what each branch's guests did and were told,
//! and, once everything stops, for garbage left behind.
//!
//! Each case doc-7 lists is a profile that makes it likely, run over many seeds; each rule of the
//! protocol has a test that turns it off and expects some seed to find the violation it prevents.

mod bucket;
mod world;

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
            other => panic!("no rule {other}"),
        }
    }
    set_rules(rules);
    let profile = match std::env::var("SIM_PROFILE").as_deref().unwrap_or("everything") {
        "everything" => Profile::default(),
        "forks" => forks(),
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

#[test]
fn forks_and_frees() {
    holds(forks(), 0..200);
}

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

#[test]
fn forks_racing_collection() {
    holds(collected(), 0..200);
}

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

#[test]
fn forks_with_children_opening() {
    holds(children(), 0..200);
}

#[test]
fn collection_needs_candidates_first() {
    needed(Rules { candidates_first: false, ..Rules::ALL }, children(), 0..20_000);
}

#[test]
fn collection_needs_forks_from_states() {
    needed(Rules { forks_from_states: false, ..Rules::ALL }, collected(), 0..20_000);
}

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

#[test]
fn takeovers_while_uploading() {
    holds(takeovers(), 0..200);
}

#[test]
fn takeovers_need_acks_after_lease() {
    needed(Rules { acks_after_lease: false, ..Rules::ALL }, Profile::default(), 0..20_000);
}

#[test]
fn takeovers_need_fences() {
    needed(Rules { fence: false, ..Rules::ALL }, Profile::default(), 0..20_000);
}

#[test]
fn takeovers_need_continuity() {
    needed(Rules { continuity: false, ..Rules::ALL }, takeovers(), 0..20_000);
}

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

#[test]
fn takeovers_racing_collection() {
    holds(taken(), 0..200);
}

#[test]
fn takeovers_need_orphans_after_successors() {
    needed(Rules { orphans_after_successor: false, ..Rules::ALL }, taken(), 0..20_000);
}

/// Case 10: extents minted after a takeover.
#[test]
fn takeovers_need_epoch_sequences() {
    needed(Rules { epoch_sequences: false, ..Rules::ALL }, takeovers(), 0..20_000);
}

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

#[test]
fn crashes_anywhere() {
    holds(crashes(), 0..200);
}

#[test]
fn forks_need_records_after_acks() {
    needed(Rules { records_after_ack: false, ..Rules::ALL }, crashes(), 0..20_000);
}

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

#[test]
fn collectors_stopping_and_racing() {
    holds(collectors(), 0..200);
}

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

#[test]
fn recoveries_racing_collection() {
    holds(recovering(), 0..200);
}

#[test]
fn recoveries_need_newest_checkpoints() {
    needed(Rules { newest_checkpoint: false, ..Rules::ALL }, takeovers(), 0..20_000);
}
