---
id: TASK-127
title: 'scripts/tests: test_every_filesystem_the_cases_measure_is_named fails on main'
status: To Do
assignee: []
created_date: '2026-10-29 01:17'
labels: []
dependencies: []
ordinal: 460000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/tests/test_bench_results.py Registry.test_every_filesystem_the_cases_measure_is_named fails on main (at 6a08c8622 and after): the cases measure fxfs-guest-fork-warm, ext4-dax-pmem-fork and ext4-pmem-fork, which the expected list lacks. scripts/test.sh exits 1 on it. Found while running the TASK-96.1 branch; not caused by it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/test.sh passes its scripts tests on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
