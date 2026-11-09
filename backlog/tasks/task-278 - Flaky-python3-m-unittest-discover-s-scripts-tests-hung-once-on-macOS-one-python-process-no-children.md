---
id: TASK-278
title: >-
  Flaky: python3 -m unittest discover -s scripts/tests hung once on macOS, one
  python process, no children
status: To Do
assignee: []
created_date: '2026-11-09 05:45'
labels:
  - flaky
dependencies: []
priority: low
type: bug
ordinal: 710000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
During TASK-276 (2026-11-09) a full run of the scripts tests (257 tests, normally ~40 s) sat for more than 10 minutes in one python3 process with no child processes and no output past the dots; killed, the next run passed in 40 s. Which test hung is unknown: the run was not verbose. Candidates are those with sockets or threads (a ResourceWarning names an unclosed 127.0.0.1 socket in a thread, test_linux_host Network fetch tests).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The hanging test is found (a verbose loop of runs) and fixed, or a per-test timeout makes a hang fail with its name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
