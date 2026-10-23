---
id: TASK-79.63
title: mutants.sh's Shuttle pass overwrites the sweep's report
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 251000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Both passes share one output directory, so the report and the summary show only the Shuttle pass's counts, and a rerun deletes the original sweep.

Where: scripts/mutants.sh:63, scripts/mutants.sh:71, scripts/mutants.sh:75, scripts/mutants.sh:15, scripts/mutants.sh:22.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.1. TASK-44.1 (In Progress) owns the mutants sweep and scripts. It does not track the Shuttle pass overwriting the sweep's report.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
mutants.sh's Shuttle pass now writes its own report to <crate>/shuttle. Before, cargo-mutants moved the sweep's <crate>/mutants.out aside and the next run deleted it. Each pass now prints its own counts. A pass that did not finish prints 'failed with status N' instead of counts, and a crate whose sweep failed gets no Shuttle pass over an earlier run's missed list. scripts/tests/test_mutants.py runs the script on a fake cargo-mutants that rotates reports as 27.1.0 does. Its 8 tests failed before these commits and pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
