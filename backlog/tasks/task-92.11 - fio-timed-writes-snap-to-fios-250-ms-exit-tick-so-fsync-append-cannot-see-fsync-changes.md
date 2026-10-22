---
id: TASK-92.11
title: >-
  fio-timed writes snap to fio's 250 ms exit tick, so fsync-append cannot see
  fsync changes
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 286000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Four write workloads are timed from fio's spawn to its exit (946-948), which lands on a 250 ms tick after about 0.14 s of startup. fsync-append is only 256 appends (1004), and its clock also covers observed()'s reads and the perf runs (1005-1021). It can only report 256/(0.137+0.25k) IOPS (the followed run gives 661.7, 402.2, 402.0), so a 30-65% change in fsync cost reads as 1.00x. rand-write moves in steps of about 18%, seq-write about 6%.

Where: guest/tests/test_cases/src/test_fs_bench.rs:737-739, guest/tests/test_cases/src/test_fs_bench.rs:946-948, guest/tests/test_cases/src/test_fs_bench.rs:970-971, guest/tests/test_cases/src/test_fs_bench.rs:989-999, guest/tests/test_cases/src/test_fs_bench.rs:1004-1021.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. fio() times from spawn to exit, and write workloads report job.secs (guest/tests/test_cases/src/test_fs_bench.rs:936-962, 976-1010). fsync-append is still 1 MiB, 256 appends, timed around observed() including its reads and perf runs (1014-1033). fio's job_runtime is not used. Related: TASK-25.1, TASK-41, TASK-79.64.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
