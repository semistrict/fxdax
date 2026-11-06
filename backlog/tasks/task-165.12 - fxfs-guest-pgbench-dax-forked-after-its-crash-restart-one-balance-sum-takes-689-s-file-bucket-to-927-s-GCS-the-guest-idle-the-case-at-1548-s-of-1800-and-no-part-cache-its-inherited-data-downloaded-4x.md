---
id: TASK-165.12
title: >-
  fxfs-guest-pgbench-dax-forked: after its crash restart one balance sum takes
  689 s (file bucket) to 927 s (GCS), the guest idle; the case at 1548 s of
  1800, and no part cache, its inherited data downloaded 4x
status: To Do
assignee: []
created_date: '2026-11-06 10:31'
labels:
  - real-bucket
  - perf
dependencies: []
parent_task_id: TASK-165
priority: high
type: bug
ordinal: 647000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the first guest suites on a real bucket (TASK-209's runs). After `pg_ctl -m immediate stop` and the restart, the case checks the balances: `select (select sum(abalance) from pgbench_accounts), ...` over the 5M-row database. That one query took 688.6 s on a file bucket (r741, us-central1-a; the case's daemon then keeps its stores in memory, 25 ms away) and 926.6 s on GCS (r744, us-east1-c, with TASK-209's fix), where the five 60 s pgbench runs before it read the same table fast. The guest was idle through it: its /proc/stat idle rose by 1,339 s over 2 vCPUs in r741's 688 s, and the device saw 36 READs, so it waits on the DAX window's faults. The case took 1,548 s of its 1,800 s limit on GCS (1,277 s on the file bucket): a slower host or bucket fails it.

Two facts point at the host's reads of what the fork inherited:
- The daemon fetched 4.3-4.7 GB and downloaded 1.9-2.3 GB for 0.51-0.54 GB of segments uploaded (get-range 28,402 in r744, 35,971 in r741): the same parts read from the bucket about four times. The case's daemon has no part cache: FXDAX_EXTENTD_CACHE is set by neither scripts/test-e2e.sh (line 188, its own daemon) nor guest/tests/test_cases/src/common.rs `spawn_daemon` (line 70), so extentd::stores::remote_cache (crates/fxdax/src/extentd/stores.rs:1216) has none, and every page-in of an extent only in the bucket is a ranged GET (crates/fxdax/src/cache.rs:399-410), again after each eviction from the extent cache. doc-14 puts a part cache on every host.
- Faults: 335,030 in r744 at 0.59 ms (198 s), 357,867 in r741 at 0.40 ms (143 s), with 4,814 waits (fault-waits): less than the query's time, so something else waits too. Not measured: which.

Not known: whether this is new with TASK-165.7 (forks only in the bucket) or older; no earlier run of this case kept its per-step times.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 What the sum after the restart waits on is found, with its time split between faults, page-ins, GETs and the guest
- [ ] #2 The guest suites' extent daemons run with a part cache of their own when their bucket is remote, as production hosts do, and a run says how many bytes each downloaded against how many it read
- [ ] #3 fxfs-guest-pgbench-dax-forked takes at most 2/3 of its limit on GCS and on a file bucket, measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
