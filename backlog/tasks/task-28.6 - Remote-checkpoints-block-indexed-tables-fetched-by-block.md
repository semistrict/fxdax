---
id: TASK-28.6
title: 'Remote checkpoints: block-indexed tables fetched by block'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 08:56'
labels: []
milestone: m-5
dependencies:
  - TASK-28.5
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 86000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The local checkpoint is one record of every extent, which a host resuming a large branch would have to fetch whole. doc-7 Checkpoints: a sorted table in 64 KiB blocks with CRCs, a block index and a trailer, so one extent costs a suffix GET, the index and one block.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Checkpoints are written in the doc-7 format, locally and to object storage, on the doc-7 cadence, and the newest is kept, with those recoveries and forks not yet opened need
- [x] #2 A host looks an extent up without loading the whole checkpoint, shown by a test counting the bytes fetched
- [x] #3 A branch state is its newest checkpoint plus the footers after it, shown by a test that compares it with the local index
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bucket gains ranged reads (bounded, suffix): simulation, Direct, object_store.
2. extent_remote::checkpoint: a state as a table of extents sorted by ID in 64 KiB blocks with CRC32C (crc-fast, iSCSI polynomial), a footer with the rest of the state, the location keys deduplicated, and a block index; encode/decode for writers and recovery; Table::open (two suffix reads) and Table::lookup (one block read).
3. Writers and readers use it; the simulation and the store tests run over it; a test counts the bytes a lookup reads.
4. Later, with TASK-28.3: the local store writes checkpoints in this format, on doc-7 cadence.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done in extent_remote: Bucket::get_range; checkpoint.rs (encode/decode/Table); writer and reader use it. Tests: checkpoints_hold_states (0..10k extents, a flipped byte is caught), lookups_read_one_block (100k extents: open reads <5% of the object, each lookup <= one 64 KiB block), the store lifecycle opens the checkpoint and looks extents up over InMemory and over RustFS (suffix and bounded ranged GETs on S3). Not yet: the local store writing checkpoints in this format and the doc-7 cadence (needs TASK-28.3). Doc-7 keeps only the newest checkpoint plus those recoveries read (the simulation showed two-newest unnecessary); AC #1 wording predates that.

c7999c9:
- The block-indexed table moved to extent_frame::table, and remote checkpoints encode through it. The local index's checkpoints are such tables, which every extent being sealed in one run, own file or remote (TASK-28.3) makes possible.
- The uploader also checkpoints at a fork and on clean shutdown, besides every 64 segments.
- Locally, checkpoints come every checkpoint_after bytes of deltas, whenever closed dead segments take a segment's room, and on opening.
AC #1 wording follows doc-7: the newest checkpoint is kept, plus those recoveries read and index files unopened forks pin; the simulation showed the two newest are not needed.
AC #3 is shown locally by closed_segments_hold_the_branchs_history and checkpoints_cover_closed_segments (rebuild equals the index's state), and in the bucket by the simulation's recovery.
Verification:
- extent_frame::table::tests
- layout::tests::checkpoints_are_tables
- checkpoint::tests::lookups_read_one_block (bytes fetched)
- remote::tests::forks_inherit_their_parents_extents (checkpoints at the fork and on shutdown)
- Workspace: macOS 1952, Linux 1962. scripts/test-rustfs.sh passes. upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Checkpoints, local and in the bucket, are now block-indexed tables in doc-7's format (extent_frame::table): sorted entries in 64 KiB blocks, each with a CRC32C, plus a footer holding the rest of the state, the keys and a block index. A host looks up one extent with the footer and one block. The local index writes its checkpoints as such tables, and a branch is its newest checkpoint plus the closed segments' footers after it. The uploader checkpoints every 64 segments, at forks and on clean shutdown. Verified by the table, checkpoint, rebuild and fork tests, RustFS, and the macOS and Linux suites.
<!-- SECTION:FINAL_SUMMARY:END -->
