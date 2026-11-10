---
id: TASK-327
title: >-
  An upload's read of a framed file copies it once more: read_whole's aligned
  buffer, then a Vec
status: To Do
assignee: []
created_date: '2026-11-10 12:05'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 728000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-317: extent_store::framed::read_whole reads a file into an Aligned buffer (O_DIRECT) and returns as_slice().to_vec(), so each object the uploader reads (read_object, Closed::bytes, stored()) is held twice for a moment as it is read, outside what the host's upload budget (FXDAX_EXTENTD_UPLOAD_MEMORY) counts: up to twice the budget if every grant reads at once. The heap profile under a backlog (heaptrack, ubu1) showed no such coincidence, but the bound documented in doc-7 (Uploads, Memory) is per object held once. Handing the Aligned buffer on as bytes::Bytes (Bytes::from_owner) to extent_remote's Body, whose bucket::create holds Bytes since TASK-317, would read it once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reading an object for its upload allocates its bytes once
- [ ] #2 A counted-heap test of an upload's read
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
