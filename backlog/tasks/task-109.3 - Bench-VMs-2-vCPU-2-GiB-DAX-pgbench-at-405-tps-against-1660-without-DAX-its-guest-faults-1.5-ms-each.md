---
id: TASK-109.3
title: >-
  Bench VMs (2 vCPU, 2 GiB): DAX pgbench at 405 tps against 1,660 without DAX,
  its guest faults 1.5 ms each
status: In Progress
assignee: []
created_date: '2026-10-26 03:14'
updated_date: '2026-10-26 03:16'
labels: []
dependencies: []
parent_task_id: TASK-109
priority: high
ordinal: 412000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Comparison run 2026-10-25 (cac2afe77, scripts/bench-gce.sh, n2-standard-8 us-east4-a, FXDAX_BENCH_GROUPS=pgbench, scale 50, 60 s runs, 3 iterations; results in the session's /private/tmp/fxdax-cmp-bench): pgbench-tps ext4 on virtio-blk 1,140, fxfs-guest 1,660, fxfs-guest-dax 405 (402-424); init 11.8 / 11.4 / 21.6 s. extentd served 159,391 FAULTs at 1,512 us each (~240 s), 271,077 MAPs at 103 us. The pgbench harness's 4 GiB DAX VM runs at ~53% of no-DAX, this 2 GiB one at ~24%: something particular to the smaller guest (its page cache and DAX window sizing, faults paging extents in from the store) costs most of the DAX case here.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The bench's DAX pgbench faults attributed: why a fault takes ~1.5 ms, and why there are 159k of them
- [ ] #2 bench-fs-fxfs-guest-dax pgbench-tps at least that of bench-fs-fxfs-guest
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: the bench's cases share scripts/test-e2e.sh's extentd, started with the default 1 GiB; its log shows critical memory pressure throughout the DAX case, and the guest's 159k faults paging evicted extents in again (the pgbench harness gives its daemon 8 GiB for this reason, test_fxfs_guest_pgbench.rs). Fix: test-e2e.sh passes FXDAX_EXTENTD_MEMORY to the daemon, bench.sh defaults it to 8 GiB. Rerunning the bench's Fxfs cases.
<!-- SECTION:NOTES:END -->
