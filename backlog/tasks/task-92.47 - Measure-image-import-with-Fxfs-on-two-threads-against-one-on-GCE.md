---
id: TASK-92.47
title: 'Measure image import with Fxfs on two threads against one, on GCE'
status: To Do
assignee: []
created_date: '2026-10-23 06:12'
updated_date: '2026-11-10 11:05'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10 (TASK-303): the GCE runs of 2026-11-08 (gce-import-bench-threads-{1,2}) had extent_store's audit compiled in. Re-measured without it on ubu1, three alternating runs each (docs/measurements/ubu1-import-bench-threads-2026-11-10): small files 13,827/s on 1 thread against 13,802/s on 2; replacing them 8,862 against 8,509; whiting out 25,814 against 27,189; large file 1,510 against 1,453 MiB/s. Two threads are no faster, so the default of 1 stands. Measured on ubu1, not GCE as this task's title says.
<!-- SECTION:NOTES:END -->
