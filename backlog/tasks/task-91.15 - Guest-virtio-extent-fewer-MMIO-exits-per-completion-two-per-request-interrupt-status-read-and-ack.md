---
id: TASK-91.15
title: >-
  Guest virtio-extent: fewer MMIO exits per completion (two per request:
  interrupt status read and ack)
status: To Do
assignee: []
created_date: '2026-10-23 18:01'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: low
ordinal: 353000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Round 6: 134,886 MMIO exits to the VMM in the DAX load (JuiceFS: 2,646), ~12 us of vCPU time each: the virtio-mmio interrupt status read and acknowledgement on each completion. Batching completions, or MSI-style signalling if Firecracker's transport allows it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 MMIO exits per completed request measured and reduced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
