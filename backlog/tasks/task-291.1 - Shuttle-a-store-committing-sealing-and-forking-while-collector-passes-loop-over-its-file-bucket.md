---
id: TASK-291.1
title: >-
  Shuttle: a store committing, sealing and forking while collector passes loop
  over its file bucket
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 09:04'
labels: []
dependencies:
  - TASK-88
parent_task_id: TASK-291
priority: high
ordinal: 692000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No Shuttle suite runs a collector. extent_store's store, its uploader and part cache, and the collector are never interleaved by a scheduler that explores orderings, so a race between a pass's listing/deleting/compacting and a store's seal, upload, acknowledgement or fork goes unfound outside the simulation, whose writers are not the store till TASK-88.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A Shuttle test in extent_store (or fxdax) runs a Store committing, sealing and forking on one thread and collector::collect looping with compact_below > 0 on another over one bucket
- [x] #2 After the threads join, every acknowledged extent of every live branch reads back its written bytes, and a fresh open of each branch recovers
- [x] #3 The test runs in the shuttle part of scripts/test.sh and fails on a mutant that drops a keep rule of the collector
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. upload::Shared gets stop() (Remote::drop uses it), so a test can stop an uploader it runs itself.
2. A lib Shuttle test module in upload.rs (feature shuttle): a bucket yielding to Shuttle before each request over extent_remote::testing::Direct; Sessions of a Store over MemDir::new() with the real uploader loop (Shared::run) on a Shuttle thread, its interval never running out; the parent writes, frees, commits and forks; once the fork is made, the child's store writes and frees what it inherited; a collector thread loops collector::collect (compact_below 0.9, grace 0).
3. After the threads join: the uploaders upload all and stop; each branch recovered from the bucket holds exactly what its store's last commit left, each extent reading its bytes.
4. Rule-off twin: a pass held up (a gate in the bucket) as it lists the bucket while the child is made; with candidates_first off the child's segment is deleted, exactly. scripts/test.sh's shuttle part runs the module (--lib shuttle_tests::).
Verify: cargo nextest run -p extent_store --features shuttle --lib shuttle_tests::
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The free search finds no loss with any of seven collector rules off (candidates_first, extent_keys_name_segments, orphans_after_successor, moves_taken_first, stores_follow_moves, forks_from_states, records_relisted) at PCT depths 3-20 and random, thousands of schedules: once a branch has a checkpoint, each pass relists records after recovery, leaving a loss two adjacent requests of room. So the rule-off twin holds a pass up with a gate (listed_while_forking), deterministic in every schedule. Verified: shuttle_tests 10/10 macOS and ubu1 (Linux); extent_store+extent_remote 416/416 macOS; upstream-changes --check ok.

Linux: ubu1 (not GCE, per the user), extent_store+extent_remote 417/417 under --cargo-profile sim at 4be497a26, excluding durable_commits_keep_the_open_segments_pages_cached_on_the_host, which fails at origin/main too on ubu1's ZFS (TASK-315).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
upload::Shared::stop() (Remote::drop uses it). upload.rs shuttle_tests: a parent store committing, sealing and forking and its fork's child's store writing, each with the real uploader loop, while collector passes (compact_below 0.9, grace 0) loop over a bucket yielding to Shuttle at each request; afterwards each branch recovered from the bucket holds exactly its last commit, bytes and all (Random + PCT, 400 schedules). The rule-off twin holds a pass up as it lists the bucket while the child is made: candidates_first off, the child's segment is deleted (exact Lost error). Free search finds no loss with seven rules off (notes say why). Verified macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
