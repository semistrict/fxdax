// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A deterministic simulation of branches in object storage (TASK-28.1): doc-7's protocol run by
//! hosts' stores, this crate's, and their uploaders (`upload`), writing, committing, forking,
//! taken over and crashing, and by deletions and collectors, over a bucket that delays, reorders
//! and fails requests (TASK-88: extent_remote's until then, whose writers made their segments
//! with `Batch`, so that the store's choice of what a segment seals, frees and forks never met a
//! takeover or a collector), checked after every step against what each branch's guests did and
//! were told, every object a branch's acknowledged segments name there, and what its live writer
//! and its store name of its guest's read back (TASK-149.32, TASK-195); and, once everything
//! stops, for garbage left behind, for tasks still waiting, a checkpoint, a deletion or anything
//! else nothing will wake, and for uploaders that upload no more what they are asked to. A host's
//! writers upload within one budget, a few PUTs at once, so that uploads wait for each other. `crates/extent_remote/mutants.sh` puts
//! back a bug of each kind, and sees a test fail.
//!
//! Each case doc-7 lists is a profile that makes it likely, run over many seeds; each rule of the
//! protocol has a test that turns it off and expects some seed to find the violation it prevents.

mod world;

use extent_remote::{Rules, set_rules};
use std::ops::Range;
use world::{Profile, Weights, World};

/// The violations a run of `seed` finds, and, if any, what the bucket held.
fn violations(seed: u64, profile: Profile) -> (Vec<String>, String) {
    let mut world = World::new(seed, profile);
    world.run();
    let description = if world.violations.is_empty() { String::new() } else { world.describe() };
    (world.violations, description)
}

/// `seeds`, as many again as `SIM_SOAK` says (1 by default) from their start.
fn soaked(seeds: Range<u64>) -> Range<u64> {
    let soak: u64 = std::env::var("SIM_SOAK").map_or(1, |soak| soak.parse().unwrap());
    seeds.start..seeds.start + (seeds.end - seeds.start) * soak
}

/// What a run of `seed` asked of the bucket, and the violations it found.
fn replay(seed: u64, profile: Profile) -> (Vec<String>, Vec<String>) {
    let mut world = World::new(seed, profile);
    world.run();
    (world.journal(), world.violations)
}

/// Runs each of `seeds` twice, expecting the same requests of the bucket, in the same order, each
/// giving the same, and the same violations: a seed replays as it ran, so that one that fails can
/// be run again to debug it (TASK-380). `SIM_SOAK` multiplies how many.
fn replays(profile: Profile, seeds: Range<u64>) {
    for seed in soaked(seeds) {
        let (first, second) = (replay(seed, profile), replay(seed, profile));
        let at = first.0.iter().zip(&second.0).position(|(first, second)| first != second);
        let at = at.unwrap_or(first.0.len().min(second.0.len()));
        assert!(
            first == second,
            "seed {seed} ran two ways: request {at} of {} and {}: {:?} and {:?}, after {:#?}",
            first.0.len(),
            second.0.len(),
            first.0.get(at),
            second.0.get(at),
            &first.0[at.saturating_sub(8)..at],
        );
    }
}

/// Runs `seeds`, failing at the first that finds a violation. `SIM_SOAK` multiplies how many.
fn holds(profile: Profile, seeds: Range<u64>) {
    holds_with(profile, seeds, &[]);
}

/// Runs `seeds` as [`holds`] does, and `found` first, seeds past them that found a bug once, which
/// every run replays.
fn holds_with(profile: Profile, seeds: Range<u64>, found: &[u64]) {
    for seed in found.iter().copied().chain(soaked(seeds)) {
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

/// A test that 200 seeds of `$profile` find no violation, and the seeds past them `$found`, each of
/// which found a bug once.
macro_rules! test_holds {
    ($name:ident, $profile:expr) => {
        test_holds!($name, $profile, []);
    };
    ($name:ident, $profile:expr, [$($found:expr),*]) => {
        #[test]
        fn $name() {
            holds_with($profile, 0..200, &[$($found),*]);
        }
    };
}

/// A test that `$rule` is needed: that one of 20,000 seeds of `$profile` without it finds a
/// violation.
macro_rules! test_needs {
    ($name:ident, $rule:ident, $profile:expr) => {
        test_needs!($name, $rule, $profile, 0..20_000);
    };
    ($name:ident, $rule:ident, $profile:expr, $seeds:expr) => {
        #[test]
        fn $name() {
            needed(Rules { $rule: false, ..Rules::ALL }, $profile, $seeds);
        }
    };
}

/// fxdax: a test that `$rule` keeps the bucket from holding for ever what no pass collects: that
/// one of 20,000 seeds of `$profile` without it leaves a reference no record names
/// ([`leaked`], TASK-395.10), a lesser failure than those [`test_needs`] finds.
macro_rules! test_leaks {
    ($name:ident, $rule:ident, $profile:expr) => {
        test_leaks!($name, $rule, $profile, 0..20_000);
    };
    ($name:ident, $rule:ident, $profile:expr, $seeds:expr) => {
        #[test]
        fn $name() {
            leaked(Rules { $rule: false, ..Rules::ALL }, $profile, $seeds);
        }
    };
}

/// Runs `seeds` with `rules`, expecting one of them to leave a reference behind, its child's
/// record naming another origin or none, garbage no pass retires (TASK-395.10).
fn leaked(rules: Rules, profile: Profile, seeds: Range<u64>) {
    set_rules(rules);
    let found = seeds.clone().find_map(|seed| {
        let (found, _) = violations(seed, profile);
        let leaked = found
            .iter()
            .any(|violation| violation.starts_with("garbage left") && violation.contains("/refs/"));
        leaked.then_some((seed, found))
    });
    set_rules(Rules::ALL);
    let (seed, found) = found.unwrap_or_else(|| panic!("no seed of {seeds:?} leaked a reference"));
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
#[ignore = "debugs one seed (SIM_SEED, SIM_PROFILE, SIM_OFF): the sim part runs the soaks"]
fn one() {
    let seed = std::env::var("SIM_SEED").map_or(0, |seed| seed.parse().unwrap());
    let mut rules = Rules::ALL;
    for name in
        std::env::var("SIM_OFF").unwrap_or_default().split(',').filter(|name| !name.is_empty())
    {
        match name {
            "pins_fences" => rules.pins_fences = false,
            "pins_old_fences" => rules.pins_old_fences = false,
            "pins_void_strays" => rules.pins_void_strays = false,
            "pins_late_strays" => rules.pins_late_strays = false,
            "pins_takings" => rules.pins_takings = false,
            "forks_after_lease" => rules.forks_after_lease = false,
            "fence" => rules.fence = false,
            "continuity" => rules.continuity = false,
            "epoch_sequences" => rules.epoch_sequences = false,
            "records_after_ack" => rules.records_after_ack = false,
            "candidates_first" => rules.candidates_first = false,
            "newest_checkpoint" => rules.newest_checkpoint = false,
            "forks_from_states" => rules.forks_from_states = false,
            "orphans_after_successor" => rules.orphans_after_successor = false,
            "tokens" => rules.tokens = false,
            "delete_tokens" => rules.delete_tokens = false,
            "footers" => rules.footers = false,
            "fork_claims_ids" => rules.fork_claims_ids = false,
            "fork_claims_landed" => rules.fork_claims_landed = false,
            "points_take_no_writer" => rules.points_take_no_writer = false,
            "forks_check_points" => rules.forks_check_points = false,
            "forks_pending" => rules.forks_pending = false,
            "collector_abandons" => rules.collector_abandons = false,
            "records_relisted" => rules.records_relisted = false,
            "records_owned" => rules.records_owned = false,
            "extent_keys_name_segments" => rules.extent_keys_name_segments = false,
            "ways_from_seals" => rules.ways_from_seals = false,
            "moves_taken_first" => rules.moves_taken_first = false,
            "stores_follow_moves" => rules.stores_follow_moves = false,
            "leases_by_takings" => rules.leases_by_takings = false,
            "passes_fenced_stop" => rules.passes_fenced_stop = false,
            "recoveries_by_origins" => rules.recoveries_by_origins = false,
            "records_kept" => rules.records_kept = false,
            "deletions_fence" => rules.deletions_fence = false,
            "deletions_reread" => rules.deletions_reread = false,
            "wal_follows" => rules.wal_follows = false,
            "wal_after_fence" => rules.wal_after_fence = false,
            "wal_continues" => rules.wal_continues = false,
            "references" => rules.references = false,
            "references_first" => rules.references_first = false,
            "clones_refer_first" => rules.clones_refer_first = false,
            "clones_record_first" => rules.clones_record_first = false,
            "references_taken_back" => rules.references_taken_back = false,
            "references_held" => rules.references_held = false,
            "references_relisted" => rules.references_relisted = false,
            "fork_points_kept" => rules.fork_points_kept = false,
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
        "recreated" => recreated(),
        "crashes" => crashes(),
        "collectors" => collectors(),
        "taken" => taken(),
        "children" => children(),
        "fresh_cells" => fresh_cells(),
        "recovering" => recovering(),
        "failed" => failed_forks(),
        "restores" => restores(),
        "forgotten" => forgotten_restores(),
        "whole_restores" => whole_restores(),
        "taking_ids" => restores_taking_ids(),
        "usurped" => usurped(),
        "deleted_forking" => deleted_forking(),
        "wals" => wals(),
        "forked_wals" => forked_wals(),
        "late_checkpoints" => late_checkpoints(),
        other => panic!("no profile {other}"),
    };
    let count = std::env::var("SIM_SEEDS").map_or(1, |count| count.parse().unwrap());
    holds(profile, seed..seed + count);
}

/// Seeds of every profile replay as they ran.
#[test]
fn seeds_replay() {
    let profiles = [
        Profile::default(),
        deleted_forking(),
        deleted_bases(),
        restores(),
        forked_wals(),
        late_checkpoints(),
        recreated(),
        failed_forks(),
    ];
    for profile in profiles {
        replays(profile, 0..20);
    }
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

/// Placements cut off taking branches back, and deleting them, with their earlier fencing tokens,
/// from the ones after them, while those write and are taken over in turn (TASK-39.7, TASK-45.1).
fn stale_placements() -> Profile {
    weights(|weights| {
        weights.takeover = 4;
        weights.open = 6;
        weights.reclaim = 4;
        weights.stale_delete = 2;
        weights.delete = 1;
        weights.close = 4;
        weights.crash = 2;
    })
}

test_holds!(stale_placements_take_nothing_back, stale_placements());

test_needs!(fencing_needs_tokens, tokens, stale_placements());

test_needs!(deletes_need_tokens, delete_tokens, stale_placements());

/// Branches forking, deleted twice at once, by a placement cut off and the latest one, while their
/// writers' fork segments are in flight and collectors run (TASK-183).
fn deleted_forking() -> Profile {
    let mut profile = weights(|weights| {
        weights.fork = 6;
        weights.open = 6;
        weights.takeover = 2;
        weights.stale_delete = 4;
        weights.delete = 3;
        weights.collect = 8;
    });
    // fxdax: sweeps alone (TASK-395.4): the searches of this profile's rules find their cases with
    // passes over every cell, none in 20,000 seeds with half the passes over one cell.
    profile.one_cell = 0.0;
    profile
}

test_holds!(branches_deleted_twice_while_forking, deleted_forking());

// The seeds that find it move with every change of the collector's requests: 32,625 and others
// once a pass read its cell before taking its lease (TASK-395.4's review), 8,363 once record_fork
// read the child's record again (TASK-395.10), and 113,765 alone of 0..120,000 once passes skipped
// unchanged cells and held them for a time (TASK-395.5, .6), and 122,627 once a pass's listing of
// its cell served its first of the records and of each branch's checkpoints (TASK-395.16; none
// of 110,000..120,000 nor 130,000..150,000; it holds with the rule). Searched from 110,000: seeds
// 36,924, 76,898 and 99,084 fail without the rule as with it, a store's log found diverged,
// TASK-403's bug, which says nothing of this rule (TASK-403 names them).
test_needs!(deletions_need_fences, deletions_fence, deleted_forking(), 120_000..130_000);

// A deletion begun as another marks the branch deleted, recovering once a pass deleted that one's
// fence and a writer's segment of a fork, taken over, landed where it was: the fork's child,
// recorded, names the extents the segment stores on their own, which the pass deleted (TASK-380).
// Its seeds of 0..20,000 went once a pass's listing of its cell served its first of the records
// and of each branch's checkpoints (TASK-395.16): none of 20,000..69,807; 69,807 fails with the rule
// as without it, a store's log found diverged (TASK-403); 83,552 the first after that holds with it.
test_needs!(deletions_need_records_reread, deletions_reread, deleted_forking(), 80_000..90_000);

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

/// fxdax: forks whose children open at once, passes over one branch's cell alone, the child's
/// among them, which may have no record yet as the pass starts, and requests slow to complete, so
/// that a pass spans the child's opening and first upload (TASK-395.4).
fn fresh_cells() -> Profile {
    let mut profile = weights(|weights| {
        weights.complete = 15;
        weights.fork = 8;
        weights.open = 12;
        weights.collect = 12;
        weights.crash = 0;
        weights.delete = 0;
        weights.stop_collector = 0;
    });
    profile.one_cell = 1.0;
    profile
}

test_holds!(forks_with_children_opening, children());

// fxdax: a pass over a child's cell begun before the child's record, which lists the child's
// segments after it read no record (TASK-395.4): every fork is to a cell of its own here, so a
// pass over the parent's cell finds no such child (upload's Shuttle test finds one within a cell).
test_holds!(passes_over_cells_being_made, fresh_cells());

test_needs!(collection_needs_candidates_first, candidates_first, fresh_cells());

test_needs!(collection_needs_forks_from_states, forks_from_states, collected());

// A host's store naming what it reads where it was acknowledged, the collector deleting that and
// the moves after, compacted again, which its writer's checkpoints followed (TASK-195).
test_needs!(stores_need_to_follow_moves, stores_follow_moves, collected());

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

// A writer acknowledging what it created, with no read of its lease (TASK-371), its creates
// failing where the collector pins the branch (`collector::pinned`, TASK-388): a later writer's
// fence, which a checkpoint covers, kept for as long as the writer taken over lives...
test_needs!(takeovers_need_fences_pinned, pins_fences, recreated());

// ...those of every epoch, not only the last: a writer taken over by two meets the first's...
test_needs!(takeovers_need_old_fences_pinned, pins_old_fences, recreated());

// ...and the void strays, earlier writers' segments landed late, where the writer taken over may
// yet create, its successor fencing past them (TASK-383)...
test_needs!(takeovers_need_void_strays_pinned, pins_void_strays, recreated());

// ...and the late ones, earlier writers' fences made after their successors', which they follow
// (TASK-383, TASK-388)...
test_needs!(takeovers_need_late_strays_pinned, pins_late_strays, recreated());

// ...and what the last writer landed after a later writer read the branch, until that one fenced.
// A pass landing in a takeover's window: rarer since a pass reads its cell before taking its lease
// (TASK-395.4's review), none of seeds 0..60,000 finding it, seeds 62,844, 135,431, 192,067,
// 208,601 and 294,195 of 60,000..300,000 did; so searched from 60,000.
test_needs!(takeovers_need_takings_pinned, pins_takings, recreated(), 60_000..80_000);

// A deleted branch's writer, taken over, acknowledging a fork only after a read of its lease: the
// deletion's fence goes with the branch.
test_needs!(deleted_forks_need_forks_after_lease, forks_after_lease, deleted_bases());

test_needs!(takeovers_need_fences, fence, Profile::default());

test_needs!(takeovers_need_continuity, continuity, takeovers());

/// Case 9: a stale writer's create of a segment in flight while a later writer's takes its place,
/// which the collector compacts and deletes, landing then: it recreates the segment, holding other
/// extents than readers whose indexes are older than the compaction look for there (TASK-79.18).
/// Takeovers hold the creates of the writers they take branches from ([`Profile::hold`]), and
/// requests complete often, so that passes of the collector finish while a branch's writer lives.
fn recreated() -> Profile {
    let mut profile = weights(|weights| {
        weights.complete = 200;
        weights.takeover = 6;
        weights.open = 6;
        weights.close = 10;
        weights.checkpoint = 6;
        weights.collect = 8;
        weights.tick = 6;
    });
    profile.hold = 1.0;
    // fxdax: sweeps alone (TASK-395.4): the searches of this profile's rules find their cases with
    // passes over every cell, none in 20,000 seeds with half the passes over one cell.
    profile.one_cell = 0.0;
    profile.options.compact_below = 0.9;
    profile.options.grace = 1;
    profile
}

// Seed 453: a writer taken over creating where a void segment was, which a pass deleted below its
// successor's fence (TASK-383).
test_holds!(stale_creates_landing_past_deletions, recreated(), [453]);

test_needs!(stale_creates_need_reads_by_footers, footers, recreated());

/// Writers checkpointing often, taken over with their checkpoints in flight, which land late, past
/// where the next writer recovered from, while collectors compact what the checkpoints name and
/// delete what they cover (TASK-175).
fn late_checkpoints() -> Profile {
    let mut profile = weights(|weights| {
        weights.complete = 80;
        weights.write = 2;
        weights.commit = 6;
        weights.close = 6;
        weights.checkpoint = 12;
        weights.takeover = 12;
        weights.open = 2;
        weights.crash = 0;
        weights.delete = 0;
        weights.fork = 0;
        weights.collect = 12;
        weights.stale_checkpoint = 6;
        weights.tick = 8;
    });
    profile.options.compact_below = 0.9;
    profile.options.grace = 1;
    profile
}

test_holds!(late_checkpoints_landing_past_takeovers, late_checkpoints());

// The rule `ways_from_seals` (TASK-175), a writer taken over checkpointing late past where the
// next recovered from, has no search here: extent_remote's simulation reached its case at one seed
// of 20,000 of `recreated` (677, where it was found), and this one, its writers the stores', at
// none of 40,000 of `recreated` or of `late_checkpoints` (TASK-88). The collector's
// `writers_reading_past_checkpoints_landing_after_they_took_over_need_ways_from_seals` shows it
// needed, and specs/bucket's mutation NoWaysFromSeals.

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

// An extent stored on its own, uploaded before the segment naming it, which the store's footer
// closed between commits covers by its next sequence number: the collector deleting it by that
// loses it once the segment lands (TASK-79.3), which only the stores' own footers reach (TASK-88).
test_needs!(
    collection_needs_extent_keys_naming_segments,
    extent_keys_name_segments,
    Profile::default()
);

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

/// Hosts crashing, keeping their write-ahead logs or losing them, and taking their branches back,
/// at once as they start again or later, while other hosts take them over and write, and the
/// segments of writers gone land late (doc-14): every open on a host with a log puts it after the
/// branch's end ([`crate::Wal`]). Nothing else: the creates of writers gone land among few
/// requests, between a recovery and its fence.
fn wals() -> Profile {
    let mut profile = weights(|weights| {
        weights.crash = 4;
        weights.open = 8;
        weights.takeover = 3;
        weights.close = 6;
        weights.fork = 0;
        weights.delete = 0;
        weights.checkpoint = 0;
        weights.collect = 0;
        weights.stop_collector = 0;
    });
    profile.restarts = true;
    profile
}

/// Logs as [`wals`] makes them, holding forks, which their next writers make again where they
/// are put, while roots take their children's IDs (TASK-174), checkpoints come and collectors run.
fn forked_wals() -> Profile {
    let mut profile = wals();
    profile.weights.fork = 3;
    profile.weights.delete = 1;
    profile.weights.checkpoint = 2;
    profile.weights.collect = 4;
    profile.weights.stop_collector = 1;
    profile.weights.usurp = 2;
    profile
}

test_holds!(logs_put_after_takeovers, wals());

test_holds!(logs_of_forks_put_after_takeovers, forked_wals());

// A host's log put after another host's acknowledged commit, which it loses.
test_needs!(logs_need_to_follow, wal_follows, wals());

// A host's log put after the branch's end as recovery found it, not after the fence: its own
// segment, landed in between, applied twice.
test_needs!(logs_need_the_end_after_the_fence, wal_after_fence, wals());

/// fxdax: logs as [`wals`] makes them, their guests writing a segment's worth or more between
/// commits, most of them: commits spanning segments, the first of which may end the commit before,
/// landing alone as a host crashes or a writer takes the branch over (TASK-92.41, TASK-345).
fn spanning_wals() -> Profile {
    let mut profile = wals();
    profile.weights.write = 18;
    profile.weights.commit = 4;
    profile
}

test_holds!(spanning_logs_put_after_takeovers, spanning_wals());

// fxdax: a host's log whose segment ending a commit and going on with the next landed alone,
// diverged, though the branch's last commit is the log's: `take` drops the next commit, durable
// on the host (TASK-345).
test_needs!(logs_need_to_continue_a_segment_landed_alone, wal_continues, spanning_wals());

/// Forks that fail and are deleted, as their hosts' cleanups delete them, those crashing partway
/// and going on, while their segments land or not, the parents' next writers give them records,
/// collectors run, and other branches take their IDs, other forks' among them (TASK-79.82,
/// TASK-79.78).
fn failed_forks() -> Profile {
    let mut profile = weights(|weights| {
        weights.fork = 6;
        weights.unfork = 6;
        weights.contest = 2;
        weights.twins = 2;
        weights.open = 4;
        weights.takeover = 2;
        weights.collect = 6;
        weights.crash = 3;
        weights.close = 2;
    });
    profile.fail_after = 0.1;
    profile
}

test_holds!(failed_forks_deleted_anywhere, failed_forks());

test_needs!(failed_forks_need_claims_of_ids, fork_claims_ids, failed_forks());

test_needs!(failed_forks_need_claims_of_what_landed, fork_claims_landed, failed_forks());

test_needs!(forks_to_one_child_need_records_owned, records_owned, failed_forks());

/// Case 11: one point restored on several hosts at once (doc-7, Points): forks of it racing each
/// other, passes of the collector, and the point's deletion, on hosts' stale copies, whose hosts
/// die at each step, what they leave opened or deleted; while the branch it names goes on, frees
/// what it held, and is deleted in turn. Requests land late, after their hosts die.
fn restores() -> Profile {
    let mut profile = weights(|weights| {
        weights.fork = 1;
        weights.point = 4;
        weights.restore = 10;
        weights.write_point = 1;
        weights.open = 8;
        weights.delete = 3;
        weights.delete_base = 2;
        weights.unfork = 3;
        weights.collect = 8;
        weights.stop_collector = 1;
        weights.crash = 3;
        weights.checkpoint = 3;
        weights.tick = 4;
    });
    profile.fail_after = 0.1;
    profile.options.compact_below = 0.9;
    profile.options.grace = 1;
    profile
}

test_holds!(points_restored_on_many_hosts_at_once, restores());

test_needs!(points_need_refuse_writers, points_take_no_writer, restores());

test_needs!(restores_need_points_read_after_their_records, forks_check_points, restores());

test_needs!(restores_need_records_saying_they_are_being_made, forks_pending, restores());

test_needs!(collection_needs_abandoning_restores, collector_abandons, restores());

/// fxdax: restores as [`restores`] makes them, half of those a crash stopped forgotten, as a host
/// deleting the directory it left unmade knows nothing of the child (TASK-395.10): what they wrote
/// is the collector's alone, a record being made given up on once its point goes, and the
/// reference written after it retired by it.
fn forgotten_restores() -> Profile {
    let mut profile = restores();
    profile.forget = 0.5;
    profile
}

test_holds!(restores_forgotten_leave_nothing_behind, forgotten_restores());

// Written first, a reference whose record never landed stays for ever.
test_leaks!(forgotten_restores_need_clones_record_first, clones_record_first, forgotten_restores());

// `references_taken_back` leaves what `uncollected` excuses (TASK-395.13): its direct test is
// extent_remote's `forks_whose_childrens_ids_are_taken_need_references_taken_back`.

/// Restores as [`restores`] makes them, fewer and among more forks, so that more take the IDs of
/// forks' children whose segments are not yet acknowledged, as guests may name any: whichever's
/// record comes first takes the ID, the other refused (TASK-173). Its rule,
/// `recoveries_by_origins`, needs a pass to list the records before the point's record and read
/// the parent's state after the restore, the point's deletion and a checkpoint, which no seed of
/// 0..20,000 of this profile or [`restores`] does: the direct test
/// `children_restored_from_points_keeping_their_origins_need_recoveries_by_origins` shows it.
fn restores_taking_ids() -> Profile {
    let mut profile = restores();
    profile.weights.fork = 4;
    profile.weights.restore = 2;
    profile.weights.crash = 1;
    profile
}

test_holds!(restores_taking_forks_ids, restores_taking_ids());

/// Roots taking the IDs of forks' children whose segments are not yet acknowledged, and deleted,
/// while the forks' parents are deleted and collectors run: a parent's writer slow to record its
/// fork finds the root's record, deleted (TASK-174).
fn usurped() -> Profile {
    weights(|weights| {
        weights.fork = 6;
        weights.open = 6;
        weights.usurp = 4;
        weights.delete = 3;
        weights.collect = 8;
        weights.checkpoint = 2;
    })
}

test_holds!(forks_ids_taken_by_roots_deleted, usurped());

test_needs!(collection_needs_records_kept, records_kept, usurped());

// fxdax: the per-cell collector's rules (TASK-395.3, TASK-395.4), every fork of the simulation's
// to a cell of its own. A base's cell's pass, the base deleted, keeps what its forks inherited only
// by their references...
test_needs!(collection_needs_references, references, deleted_bases());

// ...and its state at their fork points whole, what recovering it reads too: a fork with no
// checkpoint of its own recovers through it...
test_needs!(collection_needs_fork_points_kept, fork_points_kept, deleted_bases());

// ...and a reference stays while its child's cell holds references of its own, a generation's
// forks inheriting through it.
test_needs!(collection_needs_references_held, references_held, forks());

// A deletion's references written before it marks the parent deleted, and a clone's before it
// reads its point, have no search here: collector::tests' direct ones show them needed
// (`deletions_referring_before_they_mark_parents_deleted_need_references_first`,
// `clones_keeping_what_they_inherit_as_their_points_go_need_clones_refer_first`), as do
// specs/bucket's NoReferencesFirst and NoClonesReferFirst; references listed after the states,
// NoReferencesRelisted alone.

/// Restores as [`restores`] makes them, but no host dying, no collector stopped and no request
/// failing once it took effect: a pass that misses a restored branch then reclaims what it inherited,
/// rather than stopping first. With `restores` itself, the trajectories TASK-175 moved put the first
/// seed missing it at 64216, past the search (it was 5310).
fn whole_restores() -> Profile {
    let mut profile = restores();
    profile.weights.crash = 0;
    profile.weights.stop_collector = 0;
    profile.fail_after = 0.0;
    profile
}

// TASK-395.1: a clone's reference, listed after the states, finds what listing the records again
// finds: with references gone too, as specs/bucket's NoRecordsRelisted has it.
#[test]
fn collection_needs_records_listed_after_states() {
    let rules = Rules { records_relisted: false, references: false, ..Rules::ALL };
    needed(rules, whole_restores(), 0..20_000);
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

// Writers recovering as checkpoints land and collectors delete what they cover: with the stores'
// own segments (TASK-88), no seed of 40,000 of `takeovers` finds the case, nor of 4,000 of
// `recovering` or `everything`, which seed 1,389 of `recreated` does, a writer's extent lost.
test_needs!(recoveries_need_newest_checkpoints, newest_checkpoint, recreated());
