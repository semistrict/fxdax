---
id: TASK-359
title: >-
  Uploaded WAL segments never reached the host cache: its rename failed EXDEV
  (store on /home, cache in /tmp), a warning hid it, and every read past the
  daemon's memory went to the bucket
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 14:59'
labels: []
dependencies: []
priority: high
ordinal: 743000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1 2026-11-11 (docs/measurements/ubu1-loop-baseline-2026-11-11): extentd.log full of 'extent store: segments/... into the cache, as ...: Invalid cross-device link (os error 18)'; daemon stats get-range=5025, ~5 GB downloaded; random reads past the daemon's 1 GiB at ~30 ms (the injected 25 ms bucket latency). extent_store's leave() adopts acknowledged segments into extent_remote's Cache by rename, which can't cross filesystems; on failure it logged and deleted the file. scripts/test-e2e.sh put the cache in TMPDIR and the stores under the results directory. Fix (user): fail hard, never copy: Store::open refuses a store on another filesystem than the cache; test-e2e puts its cache in the results directory; AGENTS.md rule against silent costlier fallbacks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Store::open refuses a store whose directory is on another filesystem than the host's cache, with an error naming both; a test shows it
- [ ] #2 test-e2e's cache is on its stores' filesystem; the working-set benchmark's daemon stats show reads served from the cache, not the bucket
- [ ] #3 extent_store, extent_remote and fxdax suites pass on ubu1
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
