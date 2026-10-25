---
id: TASK-101
title: >-
  Guest DAX: with extentd's replies delayed, a store refused on a page the
  window maps zeros stops the VM (log_writes)
status: To Do
assignee: []
created_date: '2026-10-25 00:06'
labels:
  - bug
dependencies: []
priority: high
ordinal: 400000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-44.10's failpoints (GCE r82, kernel b8): FXDAX_FAILPOINTS='extentd::map=10%sleep(2);extentd::commit=20%sleep(20);extentd::write=10%sleep(5)', fxfs-guest-checks-dax: stores refused at window offsets 0x30a000, 0x30c000, 0x314000, 0x312000 (the window mapping zeros there, read-only), then 19 s later Firecracker 'Failure during vcpu run: Bad address' in log_writes (after PASS: stats). fxfs-guest-dax and pgbench-dax passed with the same delays. A store to a page mapped zeros: a hole the guest believes it mapped writably, or a range the guest wrote whose extent's mapping the host replaced with zeros (replace_kept, a freed extent) while the guest still took it for writable. The refusal that stopped it wasn't logged (powers of two).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
