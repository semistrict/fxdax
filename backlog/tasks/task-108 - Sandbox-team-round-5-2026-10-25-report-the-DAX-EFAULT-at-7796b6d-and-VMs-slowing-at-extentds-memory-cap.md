---
id: TASK-108
title: >-
  Sandbox team, round 5 (2026-10-25 report): the DAX EFAULT at 7796b6d, and VMs
  slowing at extentd's memory cap
status: To Do
assignee: []
created_date: '2026-10-25 19:06'
labels: []
dependencies: []
priority: high
ordinal: 407000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team retest at 7796b6d (before TASK-102, TASK-104 and TASK-101 reached main): ten 2 GiB DAX VMs one after another on one 8 GiB extentd, postgres:17 fork, pgbench -i -s 50 then 60 s -c 4 -j 2, RustFS 25 ms away. VM 5 died with KVM_RUN EFAULT on one page (window offset 0x33880000, 512 KiB aligned, guest rep_movs write, 38,535 failures). Refused stores reached 65,536 over the ten VMs, each a sealed extent mapped read-only, not open, across five or more branches. VMs 6-10, started after the daemon reached its cap, initialised in 53-77 s against 19-21 s below it, and ran at 407-547 tps against 991-1,080, each slower than the one before. Evidence on their box: ~/fxdax-evidence/fix-7796b6d-*.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
