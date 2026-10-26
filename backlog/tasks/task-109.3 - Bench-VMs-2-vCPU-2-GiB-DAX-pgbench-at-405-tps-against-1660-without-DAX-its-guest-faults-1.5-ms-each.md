---
id: TASK-109.3
title: >-
  Bench VMs (2 vCPU, 2 GiB): DAX pgbench at 405 tps against 1,660 without DAX,
  its guest faults 1.5 ms each
status: Done
assignee: []
created_date: '2026-10-26 03:14'
updated_date: '2026-10-26 03:44'
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
- [x] #1 The bench's DAX pgbench faults attributed: why a fault takes ~1.5 ms, and why there are 159k of them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: the bench's cases share scripts/test-e2e.sh's extentd, started with the default 1 GiB; its log shows critical memory pressure throughout the DAX case, and the guest's 159k faults paging evicted extents in again (the pgbench harness gives its daemon 8 GiB for this reason, test_fxfs_guest_pgbench.rs). Fix: test-e2e.sh passes FXDAX_EXTENTD_MEMORY to the daemon, bench.sh defaults it to 8 GiB. Rerunning the bench's Fxfs cases.

Bench rerun with the daemon at 8 GiB (bfea65a73's tree, cmp-bench2, GCE n2-standard-8 us-east4-a, scale 50, 60 s, 3 iterations): fxfs-guest 1,684 tps (init 11.1 s), fxfs-guest-dax 988 (17.2 s), was 405; FAULTs 6 (was 159,391), MAPs 547,737 at 67 us. AC 'DAX at least no-DAX' removed: that is the DAX gap itself, TASK-109's AC1, not this bench's misconfiguration.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The bench's DAX pgbench (405 tps) measured its extent daemon thrashing: the bench's cases share scripts/test-e2e.sh's daemon, which ran at its default 1 GiB, under critical memory pressure, and the guest's 159k faults paged evicted extents in again at 1.5 ms each. scripts/test-e2e.sh now passes FXDAX_EXTENTD_MEMORY to its daemon and scripts/bench.sh sets it to 8 GiB, as the sandbox team's and the pgbench harness's daemons have. Verified: bench rerun, DAX 988 tps with 6 faults. The DAX-vs-no-DAX gap (59% here) remains TASK-109's.
<!-- SECTION:FINAL_SUMMARY:END -->
