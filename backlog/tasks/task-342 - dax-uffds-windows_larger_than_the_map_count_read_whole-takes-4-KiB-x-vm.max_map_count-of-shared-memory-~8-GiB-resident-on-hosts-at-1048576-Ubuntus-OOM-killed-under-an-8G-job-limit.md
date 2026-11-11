---
id: TASK-342
title: >-
  dax::uffd's windows_larger_than_the_map_count_read_whole takes 4 KiB x
  vm.max_map_count of shared memory: ~8 GiB resident on hosts at 1,048,576
  (Ubuntu's), OOM-killed under an 8G job limit
status: To Do
assignee: []
created_date: '2026-11-11 02:08'
labels:
  - tests
dependencies: []
type: bug
ordinal: 737000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-276.6 on ubu1 (vm.max_map_count 1048576): the test's window is max_map_count + MAPPINGS_RESERVED pages, each a run of its own, and it reads them all (twice), configuring the arenas at 4x that rounded to a GiB (16 GiB). Under fxjob's default 8G memory limit it was killed (Memory cgroup out of memory: shmem-rss 8.26 GB), passing in 52 s under 20G. A host's vm.max_map_count decides the test's memory: bound it (a budget below the host's, as with_budget gives others) so that it reads past the budget without taking the host's map count in memory.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test passes under an 8 GiB memory limit on a host with vm.max_map_count 1048576, still reading more scattered pages than its budget of runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
