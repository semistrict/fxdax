---
id: TASK-28.6
title: 'Remote checkpoints: block-indexed tables fetched by block'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-17 03:54'
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
- [ ] #1 Checkpoints are written in the doc-7 format, locally and to object storage, on the doc-7 cadence, and the two newest are kept
- [ ] #2 A host looks an extent up without loading the whole checkpoint, shown by a test counting the bytes fetched
- [ ] #3 A branch state is its newest checkpoint plus the footers after it, shown by a test that compares it with the local index
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
