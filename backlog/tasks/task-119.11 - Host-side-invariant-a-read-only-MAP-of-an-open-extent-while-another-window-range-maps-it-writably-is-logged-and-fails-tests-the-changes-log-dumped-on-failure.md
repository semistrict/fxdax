---
id: TASK-119.11
title: >-
  Host-side invariant: a read-only MAP of an open extent while another window
  range maps it writably is logged and fails tests; the changes log dumped on
  failure
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 20:22'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 449000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable: map_open (crates/fxdax/src/extents.rs) is where an evacuation of an unsealed region would show; the changes recorder already logs window changes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Test runs fail on the invariant
- [ ] #2 A failing guest check dumps the host changes for its window offsets
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, tried (5fc790ffd, not on main): extentd checked each read-only MAP of an open extent against the window's writable mappings of it elsewhere (FXDAX_CHECK_OPEN_MAPS), the run failing on any. GCE r252, checks-dax, -delay, -small, crash-dax and pgbench-dax all passing: 3720 found, nearly all a small file's slice mapping a page of the open staging extent read-only (a read before the seal) while the staging extent's region maps it writably, and some a range's own space's read-only map of a piece in an open extent (map_extents' fallback). The host maps an open extent's memory, one for every mapping of it, so a read through either sees every store: no disagreement, and no data lost. The invariant as stated is not one the design keeps; an evacuation of an unsealed region would need telling apart from these on the guest's side (its gather sees only sealed regions: region->evacuate). Not pursued further without a narrower invariant.
<!-- SECTION:NOTES:END -->
