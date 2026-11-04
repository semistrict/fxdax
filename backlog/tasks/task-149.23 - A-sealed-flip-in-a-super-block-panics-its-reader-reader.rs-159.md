---
id: TASK-149.23
title: 'A sealed flip in a super-block panics its reader (reader.rs:159)'
status: Done
assignee: []
created_date: '2026-11-03 07:33'
updated_date: '2026-11-04 10:39'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
type: bug
ordinal: 569000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-153.13's 4,000-case run of corrupt_tests::flips_in_a_closed_image_with_barriers: Case { seed: 741, part: SuperBlock, sealed: true } panics at crates/fxfs/src/object_store/journal/reader.rs:159 (journal_file_checkpoint indexes checksums, which consume emptied: 'the len is 0 but the index is 0'). Reproduces on origin/main 33f235911: FXDAX_CORRUPT_SEED=741 FXDAX_CORRUPT_CASES=1 cargo nextest run -p fxfs-posix --no-capture flips_in_a_closed_image_with_barriers. Same class as TASK-149.9's reader.rs:157 and :304 fixes: consume crossing more block boundaries than the reader has read checksums for. The default 120 cases never reach it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Seed 741 gives an error from the mount, not a panic, pinned by a regression test
- [x] #2 The reader can't index checksums it hasn't read, whatever a record's length says
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The journal reader's consume refuses to skip past what it read (Inconsistent) and counts every block end a corrupt record crosses: a sealed super-block flip moving the checkpoint into a block not read emptied its checksums and panicked (seed 741). Tests test_no_skip_through_a_block_not_read, test_record_crossing_two_block_ends and the mount-level case went red with the fix out; known-panics no longer lists it. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
