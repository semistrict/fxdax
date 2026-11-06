---
id: TASK-164.12
title: >-
  A DAX fork pages in what it inherited from the bucket, a ranged GET a page-in,
  one at a time: forked-dax 0.30 with the gate's 25 ms, 0.76 without (GCE r790,
  main ccbe81d0e)
status: To Do
assignee: []
created_date: '2026-11-06 13:47'
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
- [ ] #1 Where a DAX fork's page-ins of inherited data go measured: how many GETs, of what sizes, how many at once, what the part cache and the extent cache keep and evict
- [ ] #2 forked-dax at least 0.8 and decay of dax-forked at least 0.9 in the perf gate's single phase, with its default 25 ms bucket latency, jailed, on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
