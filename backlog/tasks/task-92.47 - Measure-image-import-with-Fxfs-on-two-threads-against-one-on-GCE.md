---
id: TASK-92.47
title: 'Measure image import with Fxfs on two threads against one, on GCE'
status: To Do
assignee: []
created_date: '2026-10-23 06:12'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: task
ordinal: 332000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-92.44 (round 2a) found an import's per-file cost grows within Fxfs; fxdax::import's filesystem (Filesystem::format) runs Fxfs on one thread, a default decided on GCE. Whether two threads help the small-files layer needs the same host: FXDAX_EXAMPLE=import-bench FXDAX_EXAMPLE_ARGS="100000 256 1024 2" scripts/example-gce.sh against "... 1024 1".
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 import-bench's small-files and large-file rates with 1 and 2 Fxfs threads on GCE are recorded in the task
- [ ] #2 The import's default follows them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
