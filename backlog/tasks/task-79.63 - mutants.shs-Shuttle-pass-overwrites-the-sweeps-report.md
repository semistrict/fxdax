---
id: TASK-79.63
title: mutants.sh's Shuttle pass overwrites the sweep's report
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 06:48'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
