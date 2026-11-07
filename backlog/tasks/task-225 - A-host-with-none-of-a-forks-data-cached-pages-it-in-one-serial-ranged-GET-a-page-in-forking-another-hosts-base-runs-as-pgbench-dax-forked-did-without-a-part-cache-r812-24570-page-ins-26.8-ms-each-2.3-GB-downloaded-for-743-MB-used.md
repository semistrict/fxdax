---
id: TASK-225
title: >-
  A host with none of a fork's data cached pages it in one serial ranged GET a
  page-in: forking another host's base runs as pgbench-dax-forked did without a
  part cache (r812: 24,570 page-ins, 26.8 ms each, 2.3 GB downloaded for 743 MB
  used)
status: To Do
assignee: []
created_date: '2026-11-07 05:18'
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
- [ ] #1 A test or measurement shows the cost on a cold host before the change: GETs per page-in, bytes downloaded per byte used, and the case's time
- [ ] #2 After the change, pgbench-dax-forked on a cold host (cache emptied) stays within 2/3 of its limit on GCS and the file bucket, with GETs per page-in well below 1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
