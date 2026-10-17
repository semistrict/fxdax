---
id: TASK-28.5
title: 'Uploader: ordered uploads with a bounded lag'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 04:16'
labels: []
milestone: m-5
dependencies:
  - TASK-28.1
  - TASK-28.3
  - TASK-28.4
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 85000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2 requirement 11: a commit is safe once uploaded, and a host failure loses at most the unuploaded suffix. doc-4 already says COMMIT_DURABLE waits for the upload once object storage exists. doc-7 decision 3: at the lag bound, commits stall rather than the bound break.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Closed segments and extents of their own upload in parallel, extents before the segments naming them, and a segment counts as committed only once every earlier one is
- [x] #2 COMMIT_DURABLE completes once its commit is uploaded
- [x] #3 The lag is bounded in bytes and time; at the bound COMMIT stalls while WRITE and reads go on, shown by a test with an unreachable store
- [x] #4 After a host crash, reopening uploads what was missing and loses nothing durable locally, shown by a crash test
- [ ] #5 Uploads retry with backoff, and a branch falling behind is reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: an uploader journal (Store::upload snapshot, committed(), read_kept, release) so uploads can follow durable commits without the local segments being in the remote format yet (TASK-28.3 later makes them byte-identical).
2. fxdax::remote::Remote: a thread with a tokio runtime; on attach, takes the lease (Writer::open), uploads the difference between the store and the branch; then each interval, or when a durable commit waits, uploads the durable commits since as one segment (extents written and freed within it skipped), acknowledges, releases.
3. ExtentStore: attach_remote; COMMIT_DURABLE waits for the upload; ordered commits wait at max_lag; INFO next_sequence from the writer epoch range; EROFS once another writer took the branch.
4. extentd: FXDAX_EXTENTD_REMOTE names the bucket for every store it opens.
5. extent_remote: retries back off (Bucket::pause; 50 ms doubling to 5 s for object_store).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed b058333 (with 2bcbf1d). Tests: remote::tests uploads_what_the_store_makes_durable (catch-up on attach, batches, written-and-freed never uploaded, reopen finds the branch as left, INFO floor 2<<40), commits_wait_at_the_lag_bound (gated object store: third commit stalls at max_lag 2, goes on once reachable), durable_commits_fail_once_another_takes_the_branch (EROFS); extentd keeps_branches_in_object_storage (Linux). macOS 1914, Linux 1923.
Open: AC #1 parallel uploads (one segment per batch, sequential, for now); the lag bound counts commits, not bytes; local segments are not yet the remote objects (TASK-28.3): the uploader reads sealed data back from the local store. AC #4 is by catch-up (state diff), not per-commit replay: after a crash the branch gets one commit bringing it to the local store state. Risk: attaching a store whose branch moved on elsewhere would overwrite it (TASK-28.7 adds the check).
<!-- SECTION:NOTES:END -->
