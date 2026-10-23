---
id: TASK-79.62
title: gce.sh relies on errexit inside functions its callers run under || or if !
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
The drivers call gce.sh's functions under || and if !, where errexit stops nothing, so the functions now check each step themselves. Each host now carries a maker label with the run's random mark, and a run's trap and migrate's cleanup delete only hosts with that mark. A failing gcloud listing now fails instead of reporting 'Verified deletion'. A failed job launch ends the wait, and gce_fetch copies a partial archive but still returns tar's failure. A failed create in the next zone is deleted in that zone, and default host, results and bucket names end in the run's mark. Each fix has tests in scripts/tests/test_gce.py on a fake gcloud that failed before it, and the 79 scripts tests pass on macOS and on Linux (GCE); real gcloud is not used by these tests.
<!-- SECTION:FINAL_SUMMARY:END -->
