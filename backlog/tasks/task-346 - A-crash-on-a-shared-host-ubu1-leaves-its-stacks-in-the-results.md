---
id: TASK-346
title: A crash on a shared host (ubu1) leaves its stacks in the results
status: To Do
assignee: []
created_date: '2026-11-11 08:03'
labels:
  - testing
dependencies: []
priority: medium
ordinal: 740000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/lib/test-linux-host.sh sets kernel.core_pattern to the run's results and eu-stack's each core into RESULTS/cores (a VMM's SIGSEGV in a soak said nothing otherwise, 2026-10-31). On a host of one's own (FXDAX_HOST_PROVISIONED=1, scripts/lib/host.sh's runs on ubu1) the core pattern is every process's there, other agents' runs and apport's, so TASK-344 left it alone: a crash there keeps no stacks. Collect a run's crashes without the global pattern: e.g. apport/systemd-coredump's cores of the run's processes (by its fxjob cgroup or executable path), or a per-process pattern.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A VMM or extentd crash in a run on ubu1 leaves its threads' stacks in RESULTS/cores, without changing kernel.core_pattern
- [ ] #2 A test proves it with a process made to crash in a run
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
