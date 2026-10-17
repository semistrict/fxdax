---
id: TASK-28.7
title: Leases and takeover; opening a branch on a host without it
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 04:26'
labels: []
milestone: m-5
dependencies:
  - TASK-28.5
  - TASK-28.6
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 87000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Fencing and decision 4: takeover is explicit, a conditional update of the lease then a fence segment; the old writer finds out at its next upload and answers EROFS (doc-4). This is also what lets milestone 4 (m-6) resume a VM elsewhere.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A host takes a branch lease and fences it; the previous writer next upload fails, it answers EROFS, and nothing it uploads afterwards is read, shown by a test
- [x] #2 A branch opens on a host with an empty local store and serves reads from object storage
- [x] #3 Extents minted after a takeover never reuse an ID any earlier epoch uploaded
- [ ] #4 A fork child record is created once the parent has uploaded through the fork point
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. The uploader records, in the store directory, the last commit of the branch it uploaded and the one it is uploading (tags unique per writer: epoch<<40 | store epoch).
2. Attaching a store whose branch last commit is neither refuses: another host moved the branch on, and this store is stale.
3. An empty store of the branch (ExtentStore::create_for_branch) takes what the branch holds on attaching: reads every extent from the bucket. Lazy fetching is TASK-28.9.
4. Taking over: the lease and fence (Writer::open), then the old writer fails with EROFS (done in 28.5).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
remote::tests::opens_branches_elsewhere: a second store of the branch, empty, takes it (root and a 4 MiB extent read back), commits on; the first, reopened, refuses with "moved on elsewhere". durable_commits_fail_once_another_takes_the_branch covers AC #1 (EROFS). INFO next_sequence is the writer epoch range (AC #3). Open: AC #4 (a fork child record once the parent uploaded through the fork point) is in extent_remote (Writer::acknowledge records forks) but extentd has no fork request yet; forks opened elsewhere are refused (inherited extents need the read path). Opening on another host copies the whole branch until TASK-28.9. macOS 1915, Linux 1924.
<!-- SECTION:NOTES:END -->
