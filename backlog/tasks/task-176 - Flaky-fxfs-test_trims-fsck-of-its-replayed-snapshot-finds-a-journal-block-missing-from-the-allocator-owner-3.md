---
id: TASK-176
title: >-
  Flaky: fxfs test_trim's fsck of its replayed snapshot finds a journal block
  missing from the allocator (owner 3)
status: To Do
assignee: []
created_date: '2026-11-05 08:17'
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
- [ ] #1 The cause is known: a race in the test's fsck, or a bug in Fxfs
- [ ] #2 test_trim passes 500 stress runs under load, 4 at once
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
