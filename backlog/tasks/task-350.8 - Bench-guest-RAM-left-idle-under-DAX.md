---
id: TASK-350.8
title: 'Bench: guest RAM left idle under DAX'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 11:01'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 712000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest on a local disk caches its files in its own RAM, paid for with the VM. Under DAX it caches nothing of its files; they compete for the daemon's per-tenant memory. A VM with a large RAM and a working set inside it can do worse on fxdax, and its idle RAM is cost with no use.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Guests of 2, 8 and 16 GiB RAM rereading a working set of half their RAM, several such VMs per host, the daemon at its production memory and tenant limits, against the loop-device baseline (TASK-350.1)
- [ ] #2 Reports throughput and the host memory each uses in all (guests, daemon, page cache, ARC)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
