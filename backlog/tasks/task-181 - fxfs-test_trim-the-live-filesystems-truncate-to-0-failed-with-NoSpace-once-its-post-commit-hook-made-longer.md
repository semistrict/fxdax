---
id: TASK-181
title: >-
  fxfs test_trim: the live filesystem's truncate to 0 failed with NoSpace once,
  its post-commit hook made longer
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 09:56'
updated_date: '2026-11-06 02:22'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 637000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while settling TASK-176 (2026-11-05, macOS, branch task-160-25 on origin/main c919610e7). An experiment made test_trim's post-commit hook force a compaction of fs2 (the filesystem replayed from its snapshot) during fs2's fsck, so each hook took longer. In 1 of 400 stress runs (4 at once, /tmp/t176/locked400-3.log, iteration 9) the test's own WriteObjectHandle::truncate(&object, 0) on the live filesystem (data_object_handle.rs, the line after the comment 'This should take up more than one transaction') failed: 'truncate failed: No space'. No fsck error was printed. Not seen in 1,500 other runs of test_trim, with or without the experiment; the experiment is not in the test. Likely lead: the hook runs holding the live filesystem's commit mutex, so its compaction can't commit for as long as the hook runs, and a truncate of many extents, which borrows metadata space, may find the reservation drained. TASK-60's rule is that a shrinking truncate never fails for lack of space, so whether it is the test's doing or Fxfs's matters.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is known, with a test that fails before a fix and passes after it
- [x] #2 A shrinking truncate can't fail with NoSpace for the reason found, or the test is shown to be what starves it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find why the truncate's reservation fails.
2. Fix the test or the code, red then green.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Cause (2026-11-05)
A diagnostic in test_trim (removed since) printed the allocator's state when the truncate failed, in 4 failing runs: always pass 1, with 8,300,952 of 8,388,608 bytes used (8,317,032 once), no dead extents, and the same after waiting for the frees (allocator.freed()) and retrying: Err(NoSpace). The truncate is WriteObjectHandle::truncate with Options::default(), which reserves TRANSACTION_METADATA_MAX_AMOUNT (98,304 bytes) from the allocator, as upstream's does. Only 70-88 KiB were left. The object, sized by fxdax for more extents than a transaction can free ((TRANSACTION_ROOM / 16) * 2 blocks, every other one written, ~5.6 MiB), was on upstream's 8 MiB device. What's left free then depends on how far the live filesystem's journal and layers had grown, which compaction timing and load decide.
It's the test's doing, not Fxfs's: the lead about pending frees doesn't hold (none were pending), and the shrinking truncate a guest asks for borrows metadata space (TASK-60).

## Fix and proof
test_trim's device is 16 MiB (32768 blocks of 512 bytes), not 8.
- Red at 8 MiB: 4 of 300 NoSpace with 4 jobs at once, the same with and without TASK-176's fix (4 of 300 each), 6 of 500 in another run; and the diagnostic's 4 of 57.
- Green at 16 MiB: 500 of 500 (4 jobs at once, 125 each, during a model hunt).
Not run: GCE (Linux).

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. test_trim passes in r691 (Linux) and in the macOS run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
test_trim's second-pass truncate failed NoSpace because its fxdax-sized object nearly filled upstream's 8 MiB device; the test's doing, not Fxfs's. The device is 16 MiB now. On main: d949b41e5. Verified: red 4 of 300 at 8 MiB, 500 of 500 at 16 MiB under load, as noted; test_trim in GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
