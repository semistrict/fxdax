---
id: TASK-28.2
title: 'Extent store: O(1) branches, replacing copy_store'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 08:19'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 82000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A fork copies every file of the store today (extent_store::copy_store, used by fxdax image clones and extentd forks). The remote layout in doc-7 needs lineage: a child records its parent and fork point and reads inherited extents through the branch that minted them, and a free of an inherited extent drops only the freeing branch claim. doc-7 decision 10 puts this before uploads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Forking a store takes constant time and space whatever the store holds, shown by a test
- [ ] #2 Parent and child read every extent the parent had at the fork; frees after the fork by either do not affect the other, and an extent both free is reclaimed, shown by tests
- [ ] #3 A fork seals the parent open extents
- [ ] #4 fxdax image clones and extentd forks use it, and their tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A store directory is a lineage of branches, laid out as doc-7 lays it out in object storage: per-branch segments/, extents/, index/, and versioned records branches/<branch>.<version> (capacity, origin: parent + epoch + next sequence + index file, resolved, deleted).
2. Lineage (crates/extent_store/src/lineage.rs) holds the records and each file's claims (the branches holding extents in it). A file goes once no branch claims it and the commits that let it go are durable. Opening the store recovers every branch's claims and removes unclaimed files.
3. A fork commits durably (sealing the parent's open extents) and writes the child's record: O(1). The child's state is its parent's at the fork (replayed from the pinned parent index file) until its first opening writes its own index. Frees and moves by the parent of extents the fork inherited transfer their claims to the unopened fork.
4. Store = one branch: Piece names its segment's branch, cleaning skips shared segments, branch deletion releases claims, explicit close with an opening token.
5. Host stores (host.rs): a fork's directory is a handle naming the store and branch; a per-process registry shares one Lineage per directory. fork_host, open_host, create_host and delete_host.
6. fxdax: image clones fork (CloneKind and storage/clone.rs removed); ExtentStore open/create/fork via host stores; per-branch upload markers; ExtentStore closes its store on drop.
7. Tests: forks take constant time and space (only a record is added, and bytes are the same for 16 vs 1024 extents); parent and child free on their own (opened at once or later, across reopening); unopened forks keep what they inherited across parent frees, cleaning and checkpointing; deleting a branch; crash test with forks; host handle tests; fxdax clone and fork tests.
<!-- SECTION:PLAN:END -->
