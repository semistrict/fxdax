---
id: TASK-79.31
title: >-
  rsync -a into persistent build trees keeps old mtimes, so cargo skips changed
  files
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 06:48'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 219000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These three syncs keep source mtimes into trees whose target/ persists, so a changed file with an older mtime is not rebuilt. Tests, guest kernels and example measurements then run against the previous checkout's code.

Where: scripts/lib/lima.sh:25, scripts/test-linux.sh:16, scripts/lib/example-linux-host.sh:20, scripts/lib/test-linux-host.sh:21.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix.
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
