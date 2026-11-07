---
id: TASK-225
title: >-
  A host with none of a fork's data cached pages it in one serial ranged GET a
  page-in: forking another host's base runs as pgbench-dax-forked did without a
  part cache (r812: 24,570 page-ins, 26.8 ms each, 2.3 GB downloaded for 743 MB
  used)
status: Done
assignee: []
created_date: '2026-11-07 05:18'
updated_date: '2026-11-07 23:03'
labels: []
dependencies:
  - TASK-165.9
priority: high
type: enhancement
ordinal: 658000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found closing TASK-165.12 (r810-r813). The part cache fixes a fork of a base made on the same host: uploaded segments join the cache and are read locally. A host whose cache holds none of the fork's data, such as one forking another host's base or a fresh host after host loss, still pages in the way r812 measured with the cache off: each page-in a ranged GET, one after another (crates/fxdax/src/cache.rs:418-424, remote.read), each eviction from the extent cache another GET.

Fix to design, after TASK-165.9 (which rewrites extent_store and remote.rs): fetch whole parts into the part cache on a miss rather than the page's range, and/or read ahead along the extent, and run misses concurrently rather than serially. Measure with pgbench-dax-forked on a host whose cache was emptied, on a file bucket with 25 ms and on GCS.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test or measurement shows the cost on a cold host before the change: GETs per page-in, bytes downloaded per byte used, and the case's time
- [x] #2 After the change, pgbench-dax-forked on a cold host (cache emptied) stays within 2/3 of its limit on GCS and the file bucket, with GETs per page-in well below 1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure a cold host first: FXDAX_PGBENCH_COLD (the forks' daemon started afresh once the base's drained, its part cache empty) on pgbench-dax-forked, a file bucket at 25 ms (GCE r915) and GCS (r916). 2. Fix what it shows: the fetcher already fetches whole parts into the part cache, shares a part's GET among its readers and reads ahead along sequential reads; pgbench's random reads reset read-ahead to a part, so read an object whole into the cache on the host's first read of it, within the budget, after the read's own parts, and read ahead no part the cache holds (Cache::holds). 3. Measure the same cases after (r917 file, r918 GCS); unit tests of the read-ahead; fxdax, extent_store and extent_remote suites; push land-x3 with CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cold host before the change (land-x3-work 153e0ad0c, FXDAX_PGBENCH_COLD=1, pgbench-dax-forked, n2-standard-4, jailed, release guest kernel):
- r915, file bucket + 25 ms: pass, 405 s of 1,800 (23% of its limit); 1,987 ranged GETs for 124,583 page-ins (0.016 GETs a page-in); 102 MB downloaded for the 209 MB the page-ins read (0.49 bytes downloaded a byte used); runs 1,040 / 1,342 / 1,332 / 1,503 / 1,336 tps; faults 206 us each.
- r916, GCS (gs://echophase-fxdax-bench/cold-r916): pass, 412 s of 1,800; 2,135 ranged GETs for 123,241 page-ins (0.017); 100 MB downloaded for 198 MB read (0.51); runs 1,024 / 1,343 / 1,303 / 1,418 / 1,310 tps; faults 219 us.
- Warm against them (main 213504c4b, GCE r911): 21 ranged GETs, 2 MB downloaded, runs 1,236 / 1,364 / 1,302 / 1,417 / 1,309, faults 71 us.
So since the daemons' part cache (TASK-164.12, 165.4, 165.8) a cold host fetches each part once, whole, not a GET a page-in as r812 measured without a cache: AC #2's bounds hold before any change. What a cold host still pays is its first run, 17% slower than a warm one's, its faults three times as long: the base's parts fetched as pgbench's random reads miss them, about 2,000 GETs one at a time per vCPU, read-ahead reset to a part at each random read. r913 failed on the knob itself (the drained daemon's socket left behind, File exists), fixed in 153e0ad0c.

After (GCE r917, file bucket + 25 ms, cold, land-x3-work 1a949f41f, which read an object whole into the cache on a host's first read of it): pass, 409 s of 1,800; 2,849 ranged GETs for 121,987 page-ins (0.023); 138 MB downloaded for 189 MB read; runs 1,074 / 1,268 / 1,292 / 1,402 / 1,289 tps; faults 158 us. Against r915: faults 23% faster, the first run 3% faster, 36% more downloaded. A warm daemon's faults are 71 us: most of what a cold host pays is unpacking parts into the daemon's memory as faults first touch them, local cache or not, not its GETs. The user's decision (2026-11-07): drop whole-object read-ahead, keep only the read-ahead's skip of parts the cache holds (Cache::holds), and close on the baseline; no GCS run after (r918's ssh dropped mid-case, r919's key never propagated, the run numbers used up). What lands: read-ahead reads no part the cache holds, from the bucket or back from the cache's file (read_ahead_reads_nothing_the_cache_holds, red without it); FXDAX_PGBENCH_COLD, the cold-host knob of the pgbench cases.

Done (land-x3 in land-all on main, 878aa8848; CI 42406123209 on land-x3, 42420041819 and 42420161055 on land-all). AC #1: GCE r915 (file bucket + 25 ms) and r916 (GCS), cold (FXDAX_PGBENCH_COLD): 0.016-0.017 ranged GETs a page-in, 0.49-0.51 bytes downloaded a byte used, the case 405 s and 412 s of 1,800 (above). AC #2: met by the cold host before any change, as the user decided to close on (above): 23% of the limit on the file bucket and on GCS, GETs a page-in well below 1; what landed only removes work (read-ahead reads no part the cache holds; read_ahead_reads_nothing_the_cache_holds, red without it), and whole-object read-ahead was measured (r917) and dropped. A cold host's first run stays 17% below a warm one's, its faults three times as long: unpacking parts into the daemon's memory, not GETs. DoD: macOS clippy, workspace 2107/2107, sim 48/48 and Shuttle on land-x3; Linux in CI; scripts/upstream-changes.py --check passes on main 878aa8848; no upstream file changed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A cold host's page-ins were measured (GCE r915 file bucket, r916 GCS, with FXDAX_PGBENCH_COLD): since the part cache, about 0.017 GETs a page-in and the case at 23% of its limit, the criteria met before any change; read-ahead now reads no part the cache holds; whole-object read-ahead measured (r917: faults 23% faster for 36% more download) and dropped by the user's decision. Verified by read_ahead_reads_nothing_the_cache_holds, CI on land-x3 and land-all, and the GCE runs.
<!-- SECTION:FINAL_SUMMARY:END -->
