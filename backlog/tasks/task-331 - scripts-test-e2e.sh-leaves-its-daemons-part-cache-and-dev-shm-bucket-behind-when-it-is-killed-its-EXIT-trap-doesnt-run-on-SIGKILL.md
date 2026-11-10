---
id: TASK-331
title: >-
  scripts/test-e2e.sh leaves its daemon's part cache (and /dev/shm bucket)
  behind when it is killed: its EXIT trap doesn't run on SIGKILL
status: To Do
assignee: []
created_date: '2026-11-10 14:55'
labels:
  - testing
dependencies: []
priority: low
type: bug
ordinal: 734000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen on ubu1 2026-11-10 (reported by the TASK-310 session): /tmp, a 14 GB tmpfs, held root-owned /tmp/fxdax-e2e-cache.EphfA8 (8.5 GB) and /tmp/fxdax-e2e-cache.AVJgO1 (1.8 GB) from killed runs, no process using them; with other jobs' tmpfs files they kept fxdax.slice past its memory.high, every fxjob crawling in memcg reclaim. test-e2e.sh makes cache_dir under TMPDIR and bucket_dir under /dev/shm, removed only by its EXIT trap, which a SIGKILL (a runner's timeout, fxjob's memory limit, a killed ssh) never runs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A run killed with SIGKILL leaves no part cache or bucket of its own in /tmp or /dev/shm, or the next run removes those of runs whose processes are gone
- [ ] #2 The default cache goes where the results are (FXDAX_E2E_RESULTS), not a tmpfs, unless FXDAX_EXTENTD_CACHE says otherwise
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
