---
id: TASK-119.16
title: >-
  Crash at evacuation boundaries: stop the VM after gather, after MAP, after
  place (pause points), every acknowledged fsync generation survives
status: In Progress
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 07:01'
labels:
  - testing
dependencies:
  - TASK-119.9
parent_task_id: TASK-119
priority: medium
ordinal: 454000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra: warm-window reads agree while durable storage may be wrong.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 crash-dax variants stop at each evacuation step and verify the store
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, branch t3/task-112-6: fxfs-guest-crash-dax-gathered, -mapped, -placed: the crash test's VM stopped (reboot) once an evacuation is held at GATHERED, EVACUEES_MAPPED or PLACED, which the guest reports ('held at step N') and the host requires, then checks the store as fxfs-guest-crash-dax does. In dax-gate.sh. Pending GCE r215.
<!-- SECTION:NOTES:END -->
