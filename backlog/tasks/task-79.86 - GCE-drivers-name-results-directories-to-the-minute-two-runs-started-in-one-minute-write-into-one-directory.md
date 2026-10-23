---
id: TASK-79.86
title: >-
  GCE drivers name results directories to the minute: two runs started in one
  minute write into one directory
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-23 09:17'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 345000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.62 (round 2c), pre-existing. bench-gce.sh, example-gce.sh, gets-gce.sh and mutants-gce.sh named their default results directory to the minute (gce-*-%Y-%m-%dT%H%MZ) while default instance names go to the second, so two drivers started in the same minute ran on separate hosts but wrote into one results directory (source.txt, instance.json, logs). Fixed on r2c/scripts (default results and host names end in a per-run mark, also on test-gce's and gets' bucket prefixes).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Two drivers started in the same second write into separate results directories and bucket prefixes
- [ ] #2 scripts/tests pins it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
