---
id: TASK-165.12
title: >-
  fxfs-guest-pgbench-dax-forked: after its crash restart one balance sum takes
  689 s (file bucket) to 927 s (GCS), the guest idle; the case at 1548 s of
  1800, and no part cache, its inherited data downloaded 4x
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 10:31'
updated_date: '2026-11-07 04:39'
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
- [x] #1 What the sum after the restart waits on is found, with its time split between faults, page-ins, GETs and the guest
- [x] #2 The guest suites' extent daemons run with a part cache of their own when their bucket is remote, as production hosts do, and a run says how many bytes each downloaded against how many it read
- [x] #3 fxfs-guest-pgbench-dax-forked takes at most 2/3 of its limit on GCS and on a file bucket, measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07, found and measured (docs/measurements/real-bucket-guest-suites-2026-11-06.md, its last section). The cause: the case's daemon had no part cache (800e9e140, TASK-164.12, gave every test daemon one since, after r740-r744). With none, a page-in of an extent the store keeps only in the bucket reads it through remote.read (crates/fxdax/src/cache.rs:418-424), a ranged GET each time (extentd::stores::remote_cache, crates/fxdax/src/extentd/stores.rs:1251, none), again after each eviction from the extent cache, one after another. Reproduced on main b57d57e4a with the cache off (r812, memory bucket + 25 ms, FXDAX_EXTENTD_CACHE= on the host by hand): the sum after the restart 705.7 s, the case 1,182.0 s; over the sum's 660 s, 24,570 page-ins and 24,614 ranged GETs, 26.8 ms each (the case's 25 ms and the GET), 1.35 GB downloaded for 743 MB read; faults 6.8 s, MAPs 5.1 s; the guest's vCPUs busy 8.7 s of 1,320. With the cache: r810 (GCS) the sum 7.1 s, the case 432.3 s of 1,800 (24%), 18 ranged GETs, 1.2 MB downloaded against 4.6 GB its page-ins read; r811 (memory + 25 ms) 14.8 s, the case 467.1 s (26%), 6 GETs, 0.1 MB. Over r810's sum: 10,082 page-ins, 204 MB of the bucket's data, every one from the cache: acknowledged segments join it (TASK-165.8), so a fork's base made on the host is local.

AC2's second half: each case's daemon keeps its stats beside its log (spawn_daemon's NAME-stats.txt), and guest_results.py's summary and report say what each daemon downloaded against what it read (r813: 'extentd: downloaded 103.1 kB for 4.4 GB read of its bucket's, 143.2 MB of its store's'; the run's daemon too). Also: FXDAX_HOST_TRACE's bpftrace wrote 700 MB to its log in r810 and r811, kept now to its first MiB.

Left, production's, not this task's ACs: a host whose cache holds none of a fork's data (one forking another host's base) pages it in as r812 did, a ranged GET a page-in, one after another; a part's download, or page-ins fetched ahead, would be extent_store's or remote.rs's, TASK-165.9's to rewrite, for a task of its own if wanted.

Validation: no crate of the workspace changed (guest/tests, scripts and docs alone); macOS python3 -m unittest discover -s scripts/tests passes; the guest tests' common unit tests in Docker (2 passed); Linux by land-q's CI; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The balance sum after pgbench-dax-forked's crash restart waited on ranged GETs: its daemon had no part cache, so every page-in of data the store keeps only in the bucket was a GET, one after another (crates/fxdax/src/cache.rs:418-424), 26.8 ms each with the case's 25 ms: reproduced on main with the cache off (r812: 705.7 s, 24,614 GETs over 660 s; faults 6.8 s, MAPs 5.1 s, the guest idle). Fixed by 800e9e140 (TASK-164.12), the test daemons' part cache: the sum 7.1 s on GCS (r810, the case 24% of its limit) and 14.8 s on the file bucket (r811, 26%). Added: each daemon's downloads against its reads in the run's summary and report (r813), and a bound on the host trace's log.
<!-- SECTION:FINAL_SUMMARY:END -->
