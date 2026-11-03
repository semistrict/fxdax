---
id: TASK-160.25
title: 'Fxfs journal: its positions as one value with asserted order'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 596000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The journal on a write-once device needs its own transaction, make_room, renew_tail, tail_unrecorded and nine watermark fields in Inner. Explains TASK-139, 60's asserts, 26.6.1, 149.3, and 153.13's own flush bug. Proposal: a JournalPosition holding the watermarks, debug-asserting their order (zeroed <= zero_offset, device-flushed <= written, synced <= begun). The structural option, one journal extent record per run of sequences, needs a prototype first.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 JournalPosition with asserted order
- [ ] #2 Journal tests and model crash hunts pass with the asserts on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
