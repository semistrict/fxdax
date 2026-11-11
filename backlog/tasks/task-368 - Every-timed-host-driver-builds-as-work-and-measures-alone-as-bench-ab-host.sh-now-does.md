---
id: TASK-368
title: >-
  Every timed host driver builds as work and measures alone, as bench-ab-host.sh
  now does
status: To Do
assignee: []
created_date: '2026-11-11 22:22'
labels:
  - scripts
  - ubu1
  - perf
dependencies: []
priority: medium
ordinal: 752000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-330.4 split bench-ab-host.sh into a build job (fxjob, the work slice) and a rounds job (fxjob --bench), two jobs of one run (scripts/lib/host.sh: host_bench=0 host_job). The other timed drivers still compile inside their measurement: bench-host.sh (bench-linux-host.sh builds through scripts/bench.sh's test-e2e.sh), perf-gate-host.sh's test-host.sh runs, fork-time-host.sh, gets-host.sh, soak-host.sh. Each such build holds fxjob --bench's cores 4-7 and up to 16 GiB away from every other measurement for as long as it compiles (an hour on 4 cores for two fxdax trees). Give each a build phase (test-e2e.sh FXDAX_E2E_BUILD_ONLY into the run's work directory) run as work, then its measurement against the prebuilt artifacts.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No host driver's fxjob --bench job runs cargo or a kernel build
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
