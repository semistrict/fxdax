---
id: TASK-149.44
title: >-
  Fault injection of a truncate's MAP of zeros, to reach
  unmapped-page-read-as-hole in a VM
status: To Do
assignee: []
created_date: '2026-11-08 05:38'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 677000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.2: guest/mutants/unmapped-page-read-as-hole.patch needs a truncate's failed MAP of zeros (pages past the new end left without window pages, which a write past them grows the file over, then a load reads the kernel's zero page), which guest/sim's device fails on request. In a VM nothing makes that MAP fail: fxfs.dax_fail_maps fails ranges' maps of more than one piece, dax_fail_map_many and dax_fail_fallbacks a chunk's MAP_MANY. A parameter failing the next N MAPs of zeros (fxfs_device_map with zeros) would let a guest case reach the path, and the fsx scripts on a DAX mount with it kill the mutant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fault injection parameter fails a truncate's MAP of zeros, and a guest case with it kills unmapped-page-read-as-hole in a VM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
