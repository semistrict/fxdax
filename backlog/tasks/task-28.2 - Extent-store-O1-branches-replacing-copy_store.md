---
id: TASK-28.2
title: 'Extent store: O(1) branches, replacing copy_store'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 08:30'
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
- [x] #1 Forking a store takes constant time and space whatever the store holds, shown by a test
- [x] #2 Parent and child read every extent the parent had at the fork; frees after the fork by either do not affect the other, and an extent both free is reclaimed, shown by tests
- [x] #3 A fork seals the parent open extents
- [x] #4 fxdax image clones and extentd forks use it, and their tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed d05f3db (lineage, O(1) forks, claims, host handles, fxdax clones and ExtentStore on forks) and fa491c3 (delete_host, Storage::delete, fxdax-branch delete, docs saying rm -rf of a forked store is unsupported).
Design approved by the user on 2026-10-17: one store per lineage, laid out like the bucket, with clones as handles naming the store and branch.
Verification:
- extent_store tests 26, incl. the crash test with forks and host tests.
- fxdax storage tests: clone_closed_storage (a clone takes <= 16 KiB) and delete_clones_then_images.
- extents tests fork stores: forks_wait_for_seals_to_be_made, windows_share_extents_pages, maps_open_extents_writably.
- The extentd test two_processes_map_the_same_pages forks (Linux).
- Workspace: macOS 1944, Linux 1954. upstream-changes.py --check passes.
- The RustFS test now also deletes with fxdax-branch; it waits on a rebuild of the RustFS binary, whose target dir was removed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store's directory is now a lineage of branches, laid out like the bucket (per-branch segments, extents and index, plus versioned branch records). Store::fork seals the parent's open extents with a durable commit and writes only the child's record; the child's state is its parent's at the fork until its first opening writes its own index. Branches claim the files their extents are in; files go once unclaimed and the commits that released them are durable, and an unopened fork keeps what its parent frees. Clones and forks are handle directories naming the store; Storage::delete and fxdax-branch delete remove forks, and stores once none fork from them. Verified by the extent_store fork, crash and host tests, the fxdax clone, fork and delete tests, and the macOS and Linux workspace suites.
<!-- SECTION:FINAL_SUMMARY:END -->
