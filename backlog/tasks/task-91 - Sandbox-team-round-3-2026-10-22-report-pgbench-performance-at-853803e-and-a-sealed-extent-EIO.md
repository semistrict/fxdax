---
id: TASK-91
title: >-
  Sandbox team, round 3 (2026-10-22 report): pgbench performance at 853803e and
  a sealed-extent EIO
status: To Do
assignee: []
created_date: '2026-10-22 18:22'
labels: []
dependencies: []
priority: high
ordinal: 265000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team's report of 2026-10-22, at fxdax 853803e, one GCE n2-standard-16 at one nesting level, RustFS with 25 ms netem, postgres:17, 2 vCPUs, 4 GiB. pgbench -i -s 50 took 26-28 s against 15.8 s on their runtime (ext4 on virtio-blk over JuiceFS, guest flushes dropped) and 13.1 s on local ext4. In their 60 s pgbench -c 4 -j 2 runs, tps fell from 805 to 458 at 25 ms; at 0 ms it was 991 then 811. In 3 of 8 VMs, Fxfs's journal compaction failed with 'extent ... is sealed' and Postgres PANICked on EIO. Their measurements: COMMIT 21-43 s of each 60 s run at 25 ms; about 95k MMIO exits and 75k HLTs in the load step against virtio-blk's 2.5k; the VMM saw 203 µs per MAP against extentd's 60 µs; 1.57 GB stored for a 0.8 GB database; 16,012 of 16,822 FAULTs counted as errors. Their asks are this task's subtasks, in their order. Related: TASK-61 (round 2's slow COMMITs).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
