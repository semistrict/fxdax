---
id: TASK-44.14
title: >-
  TLA+ DaxSeal spec: regions opened while a read-only MAP of them is in flight
  (TASK-99)
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 393000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
specs/dax/DaxSeal.tla (TASK-44.2) models seals and relocation, not regions: their opening, the MAPs that grow them, readers mapping an extent read-only outside the gate. TASK-99's interleaving (a reader's read-only MAP in flight, a writer opening the region, the MAP ending read-only) is the kind a model checker finds at once. The invariant: a page the guest maps writable is one the host maps writable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The spec models regions' opening and MAPs in flight, readers' read-only MAPs outside the gate among them
- [ ] #2 With region_open checking mapped outside the region's lock, TLC finds TASK-99's interleaving; with the fix, it holds
- [ ] #3 make check-tla runs it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
