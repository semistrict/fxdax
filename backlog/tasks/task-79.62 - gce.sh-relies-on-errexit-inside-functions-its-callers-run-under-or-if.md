---
id: TASK-79.62
title: gce.sh relies on errexit inside functions its callers run under || or if !
status: To Do
assignee: []
created_date: '2026-10-21 21:00'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 250000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Two runs started in the same second share an instance name, and the loser's EXIT trap deletes the winner's host, because the owner check is ignored when errexit is suspended. A failed job launch polls for up to 8 h, and a failing gcloud listing reads as 'Verified deletion' for a host that keeps running.

Where: scripts/lib/gce.sh:238, scripts/lib/gce.sh:44, scripts/lib/gce.sh:91, scripts/lib/gce.sh:122, scripts/lib/gce.sh:77, scripts/test-gce.sh:36, scripts/test-gce.sh:63, scripts/migrate-gce.sh:62.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-25. TASK-25 created scripts/lib/gce.sh. No task covers errexit being suspended under || or if !, instance name collisions, or the false 'Verified deletion'.
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
