---
id: TASK-322
title: >-
  doc-12's durability window breaks on a disk shared by VMs' bulk writes: fsyncs
  of small files wait 4-8 s at the median while other VMs' large fsyncs are
  written
status: To Do
assignee: []
created_date: '2026-11-10 11:01'
labels:
  - perf
  - extentd
  - durability
dependencies: []
priority: high
type: bug
ordinal: 720000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-310's fxfs-guest-shared-disk with FXDAX_SHARED_DISK_FSYNC=1 on ubu1 (ext4 on a zvol, /mnt/fxdax-scratch; the file bucket and part cache on the same disk): 16 VMs, 8 at a time, each writing 768 MiB and fsyncing it, then 3,000 files of 8 KiB each fsynced. Every commit of every store waits behind the disk's queue: durable_lag p50 4,096-8,192 ms, p99 16-32 s, max 11-18 s, 3.5k of 48.3k commits within 50 ms; check_durable_median fails (doc-12: median within 50 ms). The same on main (pre-TASK-310) and with TASK-310's fix (q-fs-base-1, q-fs-main-1, 2026-11-10). The host was shared with other agents' loads (load 13-30), but the disk's own queue is the cause: the flushes' data phases sync hundreds of MB of own extents' files per VM at once (TASK-92.41), and one store's small commit waits for the device's flush behind them. doc-12 states its window for one VM on its own disk (TASK-221's pgbench); a shared disk is the sandbox hosts' case (TASK-310, LangSmith).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The case with FXDAX_SHARED_DISK_FSYNC=1 meets doc-12's median on ubu1's scratch disk, or doc-12 states the window it keeps on a shared disk, by the user's decision
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux
<!-- DOD:END -->
