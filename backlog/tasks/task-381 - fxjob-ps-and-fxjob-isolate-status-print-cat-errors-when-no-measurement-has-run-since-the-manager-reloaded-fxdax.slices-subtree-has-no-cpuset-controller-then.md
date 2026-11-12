---
id: TASK-381
title: >-
  fxjob ps and fxjob-isolate status print cat errors when no measurement has run
  since the manager reloaded: fxdax.slice's subtree has no cpuset controller
  then
status: To Do
assignee: []
created_date: '2026-11-12 11:16'
labels:
  - bug
  - ubu1
dependencies: []
priority: low
type: bug
ordinal: 762000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen on ubu1 2026-11-12 06:15 EST, right after r3305 (a --bench job) ended, while fixing TASK-378: fxdax.slice's cgroup.subtree_control was 'io memory pids', so fxdax-work.slice and fxdax-bench.slice had no cpuset.* files. fxjob ps printed 'cat: .../fxdax-work.slice/cpuset.cpus.effective: No such file or directory' (and the same for fxdax-bench.slice) with empty cpus= columns, and fxjob-isolate status, run by ps, printed 'cat: .../fxdax-bench.slice/cpuset.cpus.partition: No such file or directory'. systemd enables the cpuset controller in the subtree only while a unit sets AllowedCPUs; fxjob's EXIT trap clears the bench slice's. Both should read a missing cpuset file as the slice holding no CPUs of its own (all of the host's, no partition) rather than print errors (system-setup.d/146-fxdax_cgroups.sh on ubu1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxjob ps prints no errors with no measurement running and no cpuset controller in fxdax.slice's subtree
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
