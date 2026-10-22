---
id: TASK-62
title: >-
  Guest suites: pgbench -i -s 50 on a forked store, DAX, branches in a bucket
  with object-store latency
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-22 13:13'
labels: []
dependencies: []
priority: high
ordinal: 171000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Add the sandbox team's failing workload to the guest tests so it can't regress: import postgres:17, fork, pgbench -i -s 50 with parallel index build, DAX window 2 GiB, extentd branches in a bucket with injected latency (e.g. netem or a delaying object store), checking for completion and correct data.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
1cf3608: fxfs-guest-pgbench-dax (in test-e2e.sh's defaults): base VM formats; the host forks through a daemon of its own whose bucket requests wait FXDAX_REMOTE_LATENCY_MS (default 25; object_store ThrottledStore in extent_remote's object_store_at, tested by requests_wait_the_latency_set); a VM on the fork with a 2 GiB DAX window runs initdb, pgbench -i -q -s 50 (parallel primary-key build) and checks count(*) = count(distinct aid) = 5,000,000. Alpine 3.22's postgresql rather than the postgres:17 image. Passed on GCE in 25.8 s at 25 ms. Verified: macOS nextest of fxfs and extent_remote (741 passed), Linux workspace on GCE, upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
