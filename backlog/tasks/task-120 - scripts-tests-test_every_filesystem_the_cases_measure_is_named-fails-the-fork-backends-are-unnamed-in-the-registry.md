---
id: TASK-120
title: >-
  scripts/tests: test_every_filesystem_the_cases_measure_is_named fails; the
  fork backends are unnamed in the registry
status: Done
assignee: []
created_date: '2026-10-28 19:35'
updated_date: '2026-10-29 06:32'
labels:
  - bug
dependencies: []
priority: low
ordinal: 445000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
python3 -m unittest discover -s scripts/tests on main 56126dc9e: Registry.test_every_filesystem_the_cases_measure_is_named finds fxfs-guest-fork-warm, ext4-dax-pmem-fork and ext4-pmem-fork in test_fs_bench.rs Backend names that the expected list (and maybe bench.ORDER) lacks. Found 2026-10-28 while running DoD checks for TASK-119.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The test passes, the fork backends named where bench reports order them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Named the three fork backends of TASK-112.2 in bench_results.ORDER, after the DAX forks, and in BASELINES, each against fxfs-guest-dax-fork-warm, the gap the sandbox team asked about; the registry test lists them and checks a baseline. python3 -m unittest discover -s scripts/tests: 89 pass. Not run: the workspace's nextest (DoD #1); the change is to scripts alone.
<!-- SECTION:FINAL_SUMMARY:END -->
