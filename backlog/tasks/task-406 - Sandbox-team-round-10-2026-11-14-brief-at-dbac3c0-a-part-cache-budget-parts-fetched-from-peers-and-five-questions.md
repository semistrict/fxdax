---
id: TASK-406
title: >-
  Sandbox team, round 10 (2026-11-14 brief at dbac3c0): a part cache budget,
  parts fetched from peers, and five questions
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-14 12:15'
updated_date: '2026-11-14 14:11'
labels:
  - sandbox
dependencies: []
priority: high
ordinal: 792000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team is testing fxdax as the replacement of their JuiceFS setup. Atomic commits, forks and hashed key prefixes carry over; caching does not. Two asks: a fixed budget for the part cache, which shares an SSD with sandbox stores (today `u64::MAX`, bounded only by 4 GiB free, `remote_cache` in crates/fxdax/src/extentd/stores.rs); and parts fetched from other hosts before the bucket, as JuiceFS hosts serve each other's misses, so a popular snapshot booting on many cold hosts comes from the bucket about once per cluster. They offered their JuiceFS fork's design (~500 lines) or a PR.

Questions, with answers from the code at 123a2cbb4:
1. Without a part cache, `leave()` (crates/extent_store/src/store.rs) removes acknowledged segments, so a store's own recent writes come back from the bucket on a memory miss: is the cache meant to be on in production? Yes: doc-7 and fxdax-extentd's docs give production a cache (`FXDAX_EXTENTD_CACHE`); the guest suites run with one by default.
2. Stored bytes per tenant, cheaply? `fxdax-gc --usage [--tenant]`: one JSON line per cell and per tenant, from what each cell pass records in its lease, no whole-bucket listing (TASK-395.8). Per host, `ListStores` gives each store's own and inherited bytes (TASK-74).
3. Can a fork point at objects the collector is reclaiming? A fork keeps its parent's fork-point state in the parent's cell by a reference while it lives (TASK-395.1-.4, specs/bucket/Bucket.tla); `each_write_of_a_fork_meeting_the_collector` (crates/fxdax/src/remote/fork_faults.rs, TASK-150.1) runs a collector pass before each of a fork's writes and checks nothing a branch holds is taken.
4. S3? Supported: `FXDAX_EXTENTD_REMOTE=s3://bucket/prefix`, `AWS_*` credentials (TASK-28.4).
5. Later, a call warming a point into the part cache before a restore: a subtask.

On their side: the part cache on with a budget, no fixed MEMORY_BYTES so the governor runs, and MakePublic for platform-wide bases (shared points, `fxdaxctl share`, TASK-395.7, are the cross-tenant way to clone).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
406.1 and 406.2 done; 406.3 (warming a point into the part cache) waits for memory snapshots, as the team asked for it later. Answers to the five questions are in the description.
<!-- SECTION:NOTES:END -->
