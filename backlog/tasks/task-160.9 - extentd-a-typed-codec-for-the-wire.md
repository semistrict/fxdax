---
id: TASK-160.9
title: 'extentd: a typed codec for the wire'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 580000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each request's layout is a doc comment over args[n], encoded by hand in client.rs and decoded by hand in server.rs; stage operations are encoded twice; the daemon never refuses unknown flags (the VMM does); each handler authorizes the paths it names; flow-control bytes are counted on both sides, and the two receivers' size limits differ. doc-11 says nothing checks it. Explains the TASK-79.16 series, 79.43, 150.3, 150.4, 30df0bdff. Proposal: enum Request and StageOp with encode/decode giving today's bytes; decoding refuses unknown flags and extra payload and checks every path and branch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Requests are typed on both ends, the bytes unchanged
- [ ] #2 A WRITE with an unknown flag gets EINVAL, red before
- [ ] #3 A round-trip property test per kind
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
