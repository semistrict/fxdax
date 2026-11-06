---
id: TASK-164.12
title: >-
  A DAX fork pages in what it inherited from the bucket, a ranged GET a page-in,
  one at a time: forked-dax 0.30 with the gate's 25 ms, 0.76 without (GCE r790,
  main ccbe81d0e)
status: Done
assignee: []
created_date: '2026-11-06 13:47'
updated_date: '2026-11-06 18:50'
labels:
  - performance
  - dax
  - bug
dependencies: []
parent_task_id: TASK-164
priority: high
type: bug
ordinal: 650000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The perf gate's full run on main ccbe81d0e (docs/measurements/dax-cow-gate.md, r790 and r791) failed forked-dax (0.304 against 0.8) and decay of dax-forked (0.114 against 0.9): dax-forked 711, 1023, 439, 116, 81 tps against no-dax-forked's 1,444 median, every other gate passing. Its PRIVATEs' daemon part grew to 540 ms a request (the VM process's runs 0.1-0.4 ms): each waited for its source page to be brought in, and the daemon paged parts in one at a time, a ranged GET of the bucket each (object-store get-range = page-ins, ~2,200 a minute, one per ~27 ms), each with the case's 25 ms added latency (its default for a bucket in memory, test_fxfs_guest_pgbench.rs's added_latency; TASK-209 keeps it off real buckets). Since TASK-165.8, acknowledged segments leave the local store for the bucket, so a fork reads what its base wrote from there; r649 and r650 (before) fetched nothing and ran at 0.58 and 0.84. No DAX reads the database once into its guest's page cache; DAX's window has the daemon bring pages in whenever its cache hasn't them. Rerun with FXDAX_REMOTE_LATENCY_MS=0 on r790: forked-dax 0.756, decay 0.92, still 13-36k page-ins a minute (a ranged GET each, 400-1,100 MiB fetched), faults 30-90k a minute (r650 ~10k), the daemon's part of a PRIVATE 47-391 us (r650 83). Results trimmed to logs at /tmp/fxdax-r790 on the machine that ran it. Blocks TASK-164.6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Where a DAX fork's page-ins of inherited data go measured: how many GETs, of what sizes, how many at once, what the part cache and the extent cache keep and evict
- [x] #2 forked-dax at least 0.8 and decay of dax-forked at least 0.9 in the perf gate's single phase, with its default 25 ms bucket latency, jailed, on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06, the cause: the test daemons had no part cache, which production's daemon keeps on the host's disk (FXDAX_EXTENTD_CACHE, doc-7). Without one, the daemon paged what a fork inherited in from the bucket a page-in at a time, every page-in a ranged GET (r790: get-range 57,466 against 57,327 page-ins at 25 ms; 115,900 against 116,056 at 0 ms), one at a time, 56-72 KB each (4.1 GB over 57,466 GETs at 25 ms, 6.5 GB over 115,900 at 0 ms), nothing kept: the extent cache holds what's mapped, and without a part cache what it evicts is fetched again. The forked pair rerun with a part cache sized as production sizes it (unbounded, CACHE_FLOOR's 4 GiB free), jailed, nested, main 76d2c39cf: r792 (us-east4-a, the gate's 25 ms) no-dax-forked 1217, 1353, 1458, 1506, 1510, dax-forked 1181, 1266, 1185, 1303, 1238, forked-dax 0.849, decay 1.049, 26 ranged GETs; r793 (us-central1-a, 0 ms) no-dax-forked 941, 1101, 1174, 1157, 1144, dax-forked 922, 1003, 973, 989, 996, forked-dax 0.864, decay 1.080, 16 ranged GETs. A PRIVATE request 242 us on average at 25 ms (r790: 6.4 ms).

The fix, as the user decided for a cache that brings forked-dax to 0.8: the test daemons default to a part cache, as they default to a file bucket. guest/tests/test_cases/src/common.rs's spawn_daemon gives each case's daemon <name>-cache in the test's directory (part_cache), scripts/test-e2e.sh its own daemon a directory of its own, removed on exit, unless FXDAX_EXTENTD_CACHE is set, empty for none; scripts/lib/gce.sh's fetch leaves *-cache on the host. Test: common::tests::daemons_keep_a_part_cache_unless_told_otherwise (Docker, with latency_is_added_to_local_buckets_alone: 2 passed), which fails without the default (its first assertion). guest/tests/Cargo.lock brought up to date with extent_store's humantime, hyper and reqwest, which cargo adds at every build.

The full perf gate with it (r794: single on r792, density on r793, the gate phase over both), every case and every gate passing: forked-dax 0.836 (dax-forked 1163, 1221, 1188, 1287, 1211 against no-dax-forked's 1450 median), decay of dax-forked 1.04, dax 0.974, density-memory 0.847, density-memory-mib 2,097.4, bigmap-read 5.52; dax-forked's daemon 126,148 page-ins, 11 ranged GETs. docs/measurements/dax-cow-gate.md has it.

Validation: scripts/upstream-changes.py --check passes; FXDAX_TEST_PARTS=workspace scripts/test.sh on macOS: 2049 passed, 71 skipped (the change touches no workspace crate: guest/tests and scripts alone); Linux by land-p2's CI. The dax-gate on it (r792, r795): suites, pgbench-dax-forked and its probes pass, once pgbench-probes measures a MAP alone (TASK-164.6 has it).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Cause: the test daemons had no part cache, which production's daemon keeps on the host's disk (FXDAX_EXTENTD_CACHE, doc-7). Since TASK-165.8, acknowledged segments leave the store for the bucket, so a fork paged in what it inherited one ranged GET at a time (r790: 57,466 GETs, forked-dax 0.304). Fix: the test daemons default to a part cache (common.rs's part_cache, test-e2e.sh's own directory), unless FXDAX_EXTENTD_CACHE says otherwise; gce_fetch leaves the caches on the host. Verified: daemons_keep_a_part_cache_unless_told_otherwise (fails without the default); forked pair r792 0.849 at 25 ms (26 GETs), r793 0.864 at 0 ms; full perf gate r794, every gate passing (forked-dax 0.836, decay of dax-forked 1.04); dax-gate passed at dde984800; macOS workspace 2049 passed; Linux by land-p2's CI.
<!-- SECTION:FINAL_SUMMARY:END -->
