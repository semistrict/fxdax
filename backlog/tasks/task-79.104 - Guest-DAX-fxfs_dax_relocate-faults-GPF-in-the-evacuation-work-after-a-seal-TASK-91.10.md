---
id: TASK-79.104
title: >-
  Guest DAX: fxfs_dax_relocate faults (GPF) in the evacuation work after a seal
  (TASK-91.10)
status: To Do
assignee: []
created_date: '2026-10-24 05:06'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 375000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found landing round 2b (GCE n2-standard-8, main 709a7017 + round 2b's host-side commits, guest kernel built from the tree). fxfs-guest-checks-dax: after 'PASS: stats', at 25 s, the guest kernel oopses: general protection fault at non-canonical address 0x13e165a3879535f9, RIP fxfs_dax_relocate+0x7e, called from fxfs_dax_evacuee_place+0xf7 <- fxfs_dax_evacuate_work (kworker, workqueue fxfs_dax_grow), from cb43345f (TASK-91.10: once a seal ends, staging pages move to their ranges' own space). The check then times out at 60 s. Console saved at the review session's scratchpad fxdax-test-r2bm/guest/e2e/fxfs-guest-checks-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-guest-checks-dax passes, the evacuation running, on 10 runs on GCE
- [ ] #2 A guest check or unit test reproduces the fault before the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
