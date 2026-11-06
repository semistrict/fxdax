---
id: TASK-176
title: >-
  Flaky: fxfs test_trim's fsck of its replayed snapshot finds a journal block
  missing from the allocator (owner 3)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 08:17'
updated_date: '2026-11-06 02:22'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 636000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen twice on 2026-11-05 on branch task-160-25 (TASK-160.25), on macOS under load: 1 failure in the full run of cargo nextest -p fxfs -p fxfs-posix -p storage_device, and 1 in 6 runs of data_object_handle::tests::test_trim alone. Not reproduced after: 0 of 85 more on that branch, 0 of 76 on origin/land-157 (stress runs of 8 to 10, 4 at once, some during a model hunt). The fsck at data_object_handle.rs:5206 (fs2, the filesystem replayed from a snapshot taken in the post-commit hook) reported MissingAllocation of ExtentId { branch: 1, sequence: 55733144900337696 } 0..4096 owner 3 (the root parent store, which holds the journal's extents), the same sequence both times, ExtraAllocations of a newer owner-3 block (55733144900340686, and 55736443435220995 the first time), and AllocatedBytesMismatch [(3, 802816), (4, 106496)] vs [(3, 802816), (4, 102400)]. A wait_for_flush (journal.rs, debug_assert_not_too_long) then panicked 60 s later, a consequence: the test took 67 s, not 5. Lead: the fsck runs with no_lock while fs2's own work (its reap in the second pass, compaction, the journal's zeroing and extension in make_room) may commit, so it may see a transaction half-applied; or it is a real race in the journal's extents. TASK-160.25's changes are behaviour-equivalent in release builds, but whether they shift timing enough isn't shown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is known: a race in the test's fsck, or a bug in Fxfs
- [x] #2 test_trim passes 500 stress runs under load, 4 at once
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Compare test_trim's failure rate on task-160-25 and on origin/main, 400 stress runs each, same profile, run in parallel.
2. Find the cause; fix it in the test or the code, red then green.
3. Look for the same pattern elsewhere.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Rates (2026-11-05, macOS, nextest archives of one dev build each, run from the same target dir)
- 2 jobs at once (base = origin/main c919610e7's journal.rs, mine = task-160-25): 400 of 400 each, no failure.
- 4 jobs at once (2 base, 2 the lock fix below), 150 iterations each job: base 10 of 300 (6 fsck failures, TASK-176's own errors: owner-3 MissingAllocation and ExtraAllocations, AllocatedBytesMismatch, OrphanedObject; and 4 NoSpace, TASK-181); the lock fix 4 of 300 (4 NoSpace, no fsck failure).
So TASK-176 is pre-existing, not TASK-160.25's: the rates match, and it comes with load.

## Cause
test_trim's post-commit hook checks three filesystems with one FsckOptions, no_lock: true. The live one needs it: its hook runs inside commit_transaction, holding the commit mutex. fs2 (replayed from the snapshot) and the filesystem remounted for the trim don't, and their own work goes on as they're checked: the flush task's compactions, the graveyard's reap and trims. A commit of that work overtaking an unlocked check has it read the root parent's tree and the allocator on two sides of the commit: an old layer or journal block missing from the allocator, a new one extra, a layer file orphaned. The 60 s wait_for_flush panic after it was the compaction left in flight as the test panicked.
Upstream's test has the same no_lock (it's upstream's code); fxdax compacts sooner (METADATA_LOW_MARGIN), which makes the overlap likelier.

## Fix and proof
- test_trim: the live filesystem's check stays unlocked; fs2's and the remount's take their lock (FsckOptions::no_lock false).
- Red: an experiment forcing a compaction of fs2 during its check: unlocked, 5 of 120 failed (OrphanedObject, owner-3 ExtraAllocations); locked, 0 of 120 then 1 of 400, which was TASK-181's NoSpace, not fsck.
- Class test, fsck/tests.rs test_a_check_while_compacting_finds_nothing: 40 rounds of a commit, then fsck run while force_compact runs. With no_lock: 10 of 10 stress runs red (round 14, OrphanedObject(4, 38)); with the lock: 100 of 100 green.
- AC #2: test_trim with both fixes (this one and TASK-181's device), 4 jobs at once, 125 each: 500 of 500, during a model hunt.

## Looked around
data_object_handle.rs's four overwrite tests checked their live filesystem without the lock in the test body, nothing holding commits off, so the flush task could compact during the check: they take the lock now. filesystem.rs's FsckAfterEveryTransaction runs as a post-commit hook, so it holds the commit mutex: no_lock is right there. The NoSpace that showed up in the stress runs is TASK-181, at the same rate with and without this fix.
Not run: GCE (Linux).

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. test_trim and fsck's test_a_check_while_compacting_finds_nothing pass in r691 (Linux) and in the macOS run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
test_trim's fscks of the filesystems it mounts (replayed from the snapshot, remounted for the trim) ran without their lock while those filesystems compacted; they take it now, as do the overwrite tests. Pre-existing, not TASK-160.25's. On main: 254177bba (and d949b41e5, TASK-181). Verified: 500 of 500 stress runs under load as noted; test_trim and test_a_check_while_compacting_finds_nothing in GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
