---
id: TASK-29.3
title: 'Object storage: compressed uploads, fetches and cache'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
labels: []
dependencies:
  - TASK-29.2
parent_task_id: TASK-29
priority: high
type: feature
ordinal: 97000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Once local closed segments are compressed in the shared framing (TASK-29.2), the uploader can PUT them as they are, instead of re-encoding segments from read_kept as remote.rs does now. extent_remote::fetch fetches parts with ranged GETs by part*PART offsets and checks CRCs over raw bytes; with compressed parts it must locate parts through the object table, check the stored bytes, and decompress. The local fetch cache (extent_remote::cache) can keep the compressed parts. The collector compaction (moves objects) and checkpoints must write the same framing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Uploaded segments, own extents and moves objects are compressed in the TASK-29.1 framing
- [ ] #2 Fetches read a part with one ranged GET of its stored bytes, check its CRC and decompress it; the cache keeps parts compressed
- [ ] #3 The extent_remote simulation, the fetch and cache tests, the fxdax remote tests and scripts/test-rustfs.sh pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
