---
id: TASK-249
title: >-
  extentd's watchdog cap of 60 s behind (BEHIND,
  crates/fxdax/src/extentd/server.rs) kills healthy VMs
status: To Do
assignee: []
created_date: '2026-11-08 10:17'
labels:
  - bug
dependencies: []
priority: high
ordinal: 682000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-136.1 overlay work (thread 7a447b3d, 2026-11-01), never filed: during a DAX clone of the Linux checkout the watchdog let go of a VM in 1 of 4 runs while it was still acknowledging; with the cap lifted the VM peaked 100 s behind and finished. The cap is unchanged on main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The watchdog lets go only of a VM process that is silent, or behind without progress, shown by a test of a slow but acknowledging VM
- [ ] #2 Linux-checkout DAX clone passes 10 of 10 on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
