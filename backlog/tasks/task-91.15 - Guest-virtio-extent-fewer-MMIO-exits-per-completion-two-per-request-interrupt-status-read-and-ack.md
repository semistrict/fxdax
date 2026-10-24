---
id: TASK-91.15
title: >-
  Guest virtio-extent: fewer MMIO exits per completion (two per request:
  interrupt status read and ack)
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-23 18:01'
updated_date: '2026-10-24 00:46'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A caller spinning for its request polls its queue's used ring, with the queue's callbacks disabled while any caller polls it (EVENT_IDX: the device signals nothing then), so a request done within the spin costs no interrupt and none of its MMIO exits. 2. Count interrupts and polled completions per device (sysfs completions), printed per pgbench step. 3. Verify: interrupts per request lower in fxfs-guest-pgbench(-dax).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
06dd6a64: callers spinning poll the used ring with completions unsignalled; sysfs completions counts interrupts and polled. GCE: polled 0, ~1 interrupt per request (732,555 in five no-DAX runs), as no request type completes within the 100 us spin. Fewer exits per interrupt needs MSI-X (virtio-pci): Firecracker's PCI transport lacks the shared-memory capability the DAX window needs. Estimated cost of the exits: ~3-6% of vCPU time. Held.

MSI-X (virtio-pci with the window as a shared-memory capability) left by the user until the DAX host-side fixes land (2026-10-23).
<!-- SECTION:NOTES:END -->
