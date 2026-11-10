---
id: TASK-319
title: >-
  guest/tests is not clippy-clean: dozens of lints under -D warnings, so no gate
  covers its warnings
status: To Do
assignee: []
created_date: '2026-11-10 09:48'
labels:
  - scripts
dependencies: []
priority: low
ordinal: 724000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found during TASK-287 (2026-11-10, main 6ca5d6a80, ubu1): cargo clippy -p test_cases --features host|guest --tests -- -D warnings and -p runner fail on existing lints (needless_range_loop, double_ended_iterator_last, items_after_test_module, zombie_processes, suspicious_open_options, manual_is_multiple_of, infinite_iter, ...), so scripts/test.sh's clippy part, which now denies rustc warnings for the root workspace, leaves guest/tests out. Fix them or allow them with reasons, then add guest/tests to the clippy part.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo clippy for test_cases (host and guest features) and runner passes with -D warnings
- [ ] #2 scripts/test.sh's clippy part covers guest/tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
