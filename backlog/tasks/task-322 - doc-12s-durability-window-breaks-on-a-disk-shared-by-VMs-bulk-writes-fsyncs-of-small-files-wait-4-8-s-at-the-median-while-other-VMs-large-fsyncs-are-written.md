---
id: TASK-322
title: >-
  doc-12's durability window breaks on a disk shared by VMs' bulk writes: fsyncs
  of small files wait 4-8 s at the median while other VMs' large fsyncs are
  written
status: To Do
assignee: []
created_date: '2026-11-10 11:01'
updated_date: '2026-11-13 12:08'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10 (claude, TASK-92.41): with commits spanning segments and segments framed in place (no own files per extent, no copies), on ubu1's scratch disk with FXDAX_SHARED_DISK_FSYNC=1, 3 interleaved rounds a side under fxjob --bench: durable lag p50 8,192-16,384 ms on main vs 256-2,048 ms on the branch, p99 16-32 s vs 8-16 s, max 12-23 s vs 6-10 s, within 50 ms 1.4-3.5k vs 0.7-10k of 48.3k commits. The median moved down 4-60x but still breaks doc-12's 50 ms on a disk the VMs and their bucket share: the flushes' syncs wait behind the other VMs' writes (12.5 GiB of guest data and as much of the bucket's in ~100 s). check_durable_median still fails the case.

2026-11-13 (claude): main 4e0bf82f5 (TASK-369, TASK-384 in), fxfs-guest-shared-disk with FXDAX_SHARED_DISK_FSYNC=1 on ubu1's scratch disk, one run under fxjob --bench (16G), clean (other writers to zd0 ~1 MB/s): still FAILS doc-12's median: 7,832 of 48,272 commits within 50 ms (16.2%), p50 bucket 64-128 ms, p99 512-1024 ms, max 1,380 ms. Before: p50 4-8 s (filed), 256-2,048 ms with TASK-92.41. Flushes in the log show wait 90-115 ms behind the previous flush, or logs phases 50-165 ms (log fdatasyncs) with data and dirs near 0: what's left is the log sync behind the other VMs' writes on the shared device. The second run and the pgbench runs were stopped by the coordinator (ubu1 overloaded). In the sandboxes' long mode the same day (every VM's commits, both daemons) 61-65% were within 50 ms (TASK-330.5's note): the median holds there, not in this case.

2026-11-13 (claude): main 10f613290 on ubu1, fxjob --bench --mem 16G, clean (zd0's other writes at most 1.7 MB/s, work 0). fxfs-guest-shared-disk with FXDAX_SHARED_DISK_FSYNC=1 (11:59:44-12:01:35 UTC): FAILS doc-12's median: 1,043 of 48,271 commits within 50 ms (2.2%), p50 256-512 ms, p99 2-4 s, max 3.48 s. Its 80 slow flushes averaged 265 ms: logs 190 (the log fdatasyncs), wait 42, data 28, dirs 0. The morning's run at 4e0bf82f5 had 16.2% within 50 ms and p50 64-128 ms; nothing on the store's commit path landed in between (TASK-395.5's bucket listing, fxjob, fxdax-gc --usage), so one run says little: the spread is the shared device's queue from run to run, and the median breaks in both. fxfs-guest-pgbench (12:01:35-12:07:33): passes: 552,021 of 678,614 within 50 ms (81.3%), p50 <=32 ms, p99 <=128 ms, max 166 ms, tps 2,870-3,043. In the sandboxes' long mode the same day, 59-65% within 50 ms. AC #1 not met: on a disk VMs share with bulk fsyncs, the flush's log syncs still wait behind the device's queue.
<!-- SECTION:NOTES:END -->
