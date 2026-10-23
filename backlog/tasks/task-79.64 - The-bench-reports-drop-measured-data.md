---
id: TASK-79.64
title: The bench reports drop measured data
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
ordinal: 252000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Any failed case stops bench.sh before the report, leaving comparison.md empty. bench-history.py drops the fork and followed backends and orders runs by name. bench-report.py builds rows only from the first backend's workloads.

Where: scripts/bench.sh:23, scripts/bench.sh:33, scripts/lib/bench-linux-host.sh:51, scripts/bench-gce.sh:94, scripts/bench-history.py:26, scripts/bench-history.py:106, scripts/bench-report.py:222, scripts/bench-report.py:233.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-25, TASK-35. No task covers bench.sh stopping before the report, bench-history.py dropping backends or sorting by name, or bench-report.py using only the first backend's workloads.
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
