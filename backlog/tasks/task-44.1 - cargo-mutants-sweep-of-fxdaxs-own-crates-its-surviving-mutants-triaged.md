---
id: TASK-44.1
title: 'cargo-mutants sweep of fxdax''s own crates, its surviving mutants triaged'
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
updated_date: '2026-10-24 00:53'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 130000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Mutation testing of the existing code, the cheapest of the techniques: cargo-mutants (sourcefrog/cargo-mutants, MIT) rewrites the code a mutation at a time and runs the tests against each; a mutant no test kills is code the tests don't check. fxdax's own crates: extent_frame, extent_store, extent_remote, fxdax (not the vendored Fuchsia ones, whose tests are upstream's). store.rs has 2,300 lines and one inline unit test (more in crates/extent_store/tests); extent_remote's collector has none of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo-mutants has run over extent_frame, extent_store, extent_remote and fxdax with cargo nextest, and its report (caught, missed, timeouts, unviable, per crate) is kept in docs/measurements
- [ ] #2 Each missed mutant is killed by a new test, or recorded as equivalent with why, or filed as a follow-up task
- [ ] #3 How to run it is a script or make target, documented, with its run time
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. cargo install cargo-mutants (27.1.0, MIT).
2. cargo mutants -p extent_frame -p extent_store -p extent_remote -p fxdax --test-tool nextest --jobs 4 (3,589 mutants), output kept.
3. Triage missed mutants per file: a test that kills each, or equivalent with why, or a follow-up task.
4. A script/make target to rerun it (whole or per file), its time recorded; the report in docs/measurements.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sweep (cargo mutants -p extent_frame -p extent_store -p extent_remote -p fxdax --test-tool nextest --jobs 3, output ~/.cache/fxdax/mutants): 3,617 mutants (fxdax 1,758, extent_store 984, extent_remote 604, extent_frame 271). Slow: extent_remote's tests take ~77 s a mutant (its simulation), so the whole sweep is about a day; triaged per crate as each finishes.

extent_frame (done): 174 caught, 32 missed, 64 unviable, 1 timeout. Killed by tests in 85a6897 (segment footer/tag/extent_key, table blocks' fullness and index checks, stored_in's checks behind the CRC, raw unpack length); rerun with --iterate: 27 caught, 1 timeout, 5 equivalent (pack_part's guard x3, table encode > as >=, tag | as ^; why in the commit). fxdax-gc (extent_remote's bin): 5 missed, killed by 3f34282 (parse() tested; the program run over memory:///).

2026-10-18: the full sweep (background, ~/.cache/fxdax/mutants) stopped when its session ended, partway through extent_remote; to rerun with a script that also runs the Shuttle tests (cargo-mutants can't run both feature sets at once: a pass without features, then --iterate --features shuttle -- -E 'test(/shuttle_tests::/) | binary(shuttle)' over the missed, as TASK-42.2's views sweep did). Found outside the views code by TASK-42.2's sweep, to triage here: store.rs open_in's Recovered {next_sequence: 0, segment: 0} at a fork's first opening (deleting either survives); upload's Uploads{durable}; release's unreleased.retain(> epoch) as == or >= (unreleased_bytes untested at the boundary).

2026-10-19: scripts/mutants-gce.sh (+ scripts/lib/mutants-linux-host.sh; FXDAX_MUTANTS_SHARD for cargo-mutants' --shard) runs the sweep on a GCE host, where the Linux-only code builds: extent_store's sweep running (fxdax-test-mutants-store, us-west1-b). From the partial sweep of 10-18, extent_remote's missed mutants killed by tests (455478b, 8a5666d): the simulation checks compaction (no named object under compact_below without its moves) and that a quiesced collector reports what it deleted and compacted; cache eviction order, error paths, capacity, file size, free bytes; the fetch budget's limits and give-backs; retry pauses; a child's one record; a deleted branch's record and lease. Rerun over collector.rs (85 mutants): 59 caught, 7 timeouts, 8 unviable, 11 missed before the report check, those of collect/compact's counters then killed. Still missed, to triage: live()'s deleted-record guard on Error::Lost (the simulation never deletes a branch between reading its record and recovering it), compact's < as <= (float boundary, equivalent), its || as && of 'moves exist' (likely killed by the report check: verify in the GCE sweep), doomed's Record deleted&&!forked as || (records touched by recovery spare live ones: equivalent unless a deleted fork's record matters), orphan()'s epoch arithmetic (quiesce moves every branch to a later epoch: of < current covers them).

extent_store (2026-10-19, pushed b42422d): the GCE sweep (docs/measurements/gce-mutants-extent_store-2026-10-19: 1,260 mutants, 830 caught, 210 unviable, 11 timeouts) left 209, 196 after the Shuttle pass. Tests killed 84: tests/store.rs (reservations, capacity, parents, newer, flush_pending, places, releases, uploads, closed segments' last, cleaning, commits' lists, empty segments, live bytes on sealing, frames past 16 MiB, zero-byte writes, deltas checkpointing, forks' frees, forks of two points, reads past an end, damaged index records, the unpacked cache, packed read ranges, lineage flags), tests/views.rs (duplicates, NotHeld), tests/host.rs (handles, deletion, taking the directory, removal), tests/lineage.rs (a seeded model of branches' files across forks, frees, cleaning, deletions and reopening, in memory and on the host), the Shuttle race with an abandoned write. The 112 left are triaged in triage.md there: 73 equivalent, 7 in the test double MemDir, 13 needing fault injection or a power loss, 19 follow-ups (listed, unfiled: they need the user's say to become tasks). extent_remote's sweep and fxdax's (6 shards) run on GCE.

2026-10-19 evening: sweeps stopped by the user (too slow; production asks first). Done: extent_remote whole (docs/measurements/gce-mutants-extent_remote-2026-10-19/triage.md); fxdax shard 0 (415 mutants) and its survivors killed or explained; shard 1 to 312 mutants (host limit); shards 2 and 3 partial (their survivors seen in the logs were handled: tests in be96768 and before); shards 4, 5 not run. Lessons: ~2 min a mutant on fxdax, ~4 min on extent_remote; 2-3 jobs per n2-standard-8; cargo-mutants' tree copies now go on disk (TMPDIR), the Shuttle pass reruns only the missed, test code excluded (.cargo/mutants.toml). To resume: shards 2-5 with scripts/mutants-gce.sh, then the fxdax triage.md.

2026-10-23 audit: back to To Do, mutation sweeps stopped by the user on 2026-10-19. AC3 met (scripts/mutants.sh, scripts/mutants-gce.sh). AC1 partial: extent_store and extent_remote reports in docs/measurements (2026-10-19), extent_frame's in these notes only, fxdax shards 0-1 done (shard 1 stopped at 312 mutants), 2-3 partial, 4-5 not run. AC2: extent_store's 19 follow-ups not filed, fxdax's triage not started.
<!-- SECTION:NOTES:END -->
