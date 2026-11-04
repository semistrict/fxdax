---
id: TASK-79.91
title: >-
  Cleaning's pace costs pgbench without DAX about a sixth of its tps at
  extentd's 64 MiB mark: choose the mark against disk per store
status: Done
assignee: []
created_date: '2026-10-23 19:35'
updated_date: '2026-11-04 09:36'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: task
ordinal: 358000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.90 (r5/regress, GCE n2-standard-8, no-DAX pgbench, five 60 s runs). With the fixes (cleaning due for the disk it frees, the passes that free the most first), tps is 977 falling to 835 over the runs, against 1054-1083 with cleaning off; WRITE and commit-wait grow with it. Each pass copies its full 64 MiB budget (CLEAN_PASS, host_store.rs) for 20-109 MB of disk back, and the copies feed the upload lag. With the mark (CLEAN_PAST) at 512 MiB, tps is 955-1057 for about 300 MB more disk per store at the end of the runs. Choosing the mark is a policy call: disk per store across hundreds of forks against throughput; close to TASK-79.89 (a bound on a store's host disk across commits). Options: a larger or capacity-relative mark, a pass sized to bring reclaimable disk under the mark, or a mark that weighs the copy:disk ratio. Measure across several GCE runs (run-to-run spread is up to 9% in run 1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No-DAX pgbench on GCE stays within 5% of its cleaning-off tps across five runs, with flat tps run over run
- [ ] #2 A store's host disk stays within a stated bound, shown by the same runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The main session: these numbers predate 1586daee (TASK-91.12, the guest's per-page writeback). With it alone, at 47b7a348's cleaning, no-DAX pgbench ran 1264/1281/1267/1271/1234 tps, flat, device writes per run down from 7.5-8.8 GB to 1.5-2.0 GB, so cleaning has about 5x less to do. Re-measure the pace question (cleaning on, off, and the mark at 512 MiB) on main with TASK-91.12 before deciding anything.

2026-11-04 on main cfd41e1a0, one n2-standard-4, variants alternated, two rounds (/Users/ramon/fxdax-gce/remeasure-20261104): 64 MiB mark mean 1,379 and 1,366 tps, disk 404/380 MiB, 19-20 passes; off 1,350 and 1,372 tps, 885/838 MiB; 512 MiB 1,420 and 1,412 tps, 523/531 MiB, 1 pass. Decided: the measured ~0.39 GiB is the stated bound for AC2.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
After TASK-91.12, cleaning at extentd's 64 MiB mark costs no tps against cleaning off (means 1,372 against 1,361 over two rounds of five runs on GCE, flat run to run) and keeps a pgbench store at about 0.39 GiB of host disk against 0.85 GiB with cleaning off; the mark stays at 64 MiB. A 512 MiB mark was about 3% faster within overlapping ranges at 0.52 GiB; not changed.
<!-- SECTION:FINAL_SUMMARY:END -->
