---
id: TASK-92.55
title: >-
  Opening a fork of a large base costs ~0.5 s of zstd on the remote thread,
  growing with the base
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-24 03:39'
updated_date: '2026-11-08 04:51'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 370000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b (TASK-92.17's test counting_followers_walks_no_extents). 100 forks of a base of 32,768 extents opened through the daemon on a GCE host (debug build) took ~55 s, about 0.5 s a fork (2 s before unpacked parts were zeroed at once). perf puts it on the 'fxdax remote' thread in ZSTD_compress2 / ZSTD_decompressSequences as each fork opens, so a fork's opening compresses or reads data that grows with its base, against TASK-34's rule that a connection's cost does not grow with its base.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening a fork costs the same at any base size, shown by a test counting what an open compresses and reads
- [ ] #2 100 forks of a 32,768-extent base open in a time flat in the base, on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 sweep, at 1a3391d7c: the zstd is gone (TASK-165.x): a fork's opening unpacks no extent and reads no stored byte (stats().unpacks, read_stored_bytes: 0 at either base size), but its time still grows with the base. Measured with a test opening forks of a base of 1,024 and of 32,768 extents through the daemon (each extent one BLOCK, committed 1024 at a time): locally (Linux in Docker, debug) ~6 ms against ~56 ms a fork (20 forks); on GCE r1021 (n2-standard-4, debug, 100 forks) 6.0 ms against 124.5 ms, the 'fxdax remote' thread at 100% CPU for the whole 12.6 s. Profile (perf, local with full call chains; GCE leaves agree): per fork, the base's uploader, acknowledging the segment with the fork, writes a checkpoint of the base's whole state (upload.rs acknowledged -> writer.checkpoint -> extent_remote::checkpoint::encode, cloning the BTreeMap<ExtentId, Location> and encoding its table), and the child's opening recovers from that checkpoint (start -> extent_remote::recover -> recovery::checkpoint -> checkpoint::decode, building the same map; ~4% zstd of the checkpoint object). Both O(base extents). User's decision 2026-11-08 (OPEN_QUESTIONS.md b0d7d00d2): do it: checkpoints that point to the parent's instead of copying it, decoded state shared between forks.
<!-- SECTION:NOTES:END -->
