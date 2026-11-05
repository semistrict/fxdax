---
id: TASK-165.7
title: >-
  Forks, points and deletion only in the bucket: no local lineage, layers,
  ledger, branch records, handles or claims
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 17:15'
labels: []
dependencies:
  - TASK-165.3
  - TASK-165.5
  - TASK-165.6
  - TASK-164
  - TASK-160.8
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 629000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 7. The local lineage and remote.rs's claim, marker and take_parent machinery keeping it agreeing with the bucket are where TASK-79.5, 79.20, 79.71, 79.72, 79.77, 79.78, 79.82, 79.85 and 79.95 were. A fork becomes the parent's commit naming the child and the child's record; a child is a store opened from the bucket. Starts after TASK-164 and TASK-160.8 (extents.rs and extentd/stores.rs change in all three); lands in one batch with TASK-165.8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fork, a fork of a point, a point and a deletion go through extent_remote::branch and the parent's commit alone; the child's directory is made once its record exists, and opening it recovers from the bucket, sharing the parent's state in memory on the fork's host
- [ ] #2 extent_store's lineage.rs, layers.rs, ledger.rs, branch_records.rs and host.rs's handles are gone, and remote.rs's Claim, origin-* files, take_parent and unfork
- [ ] #3 remote.rs's fork tests, fork_faults.rs, restore_faults.rs and the extentd fork and restore tests pass rewritten as doc-14's table says; each removed test is listed in the notes with what replaced it
- [ ] #4 A fork of a 10 GiB base on GCE is timed before and after, and recorded in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
From TASK-165.6: an image's own file bucket is <image>/bucket, inside its directory, which its clones' handles name. That holds only while the local lineage keeps an image's directory as long as its clones live; once forks are only in the bucket, a deleted image's directory goes with its bucket under its clones. Move it (beside the image, or a bucket the caller names) in this step.
<!-- SECTION:NOTES:END -->
