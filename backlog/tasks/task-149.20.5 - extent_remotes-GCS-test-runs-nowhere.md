---
id: TASK-149.20.5
title: extent_remote's GCS test runs nowhere
status: To Do
assignee: []
created_date: '2026-11-07 13:37'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: low
type: task
ordinal: 670000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. extent_remote store::tests::gcs (ignored: needs FXDAX_GCS_URL and credentials, TASK-205) is run by no script or job; RustFS's and Azurite's now run in CI's object-stores job. GCE hosts have credentials for the run's bucket.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A script or the nightly runs it against a GCS bucket, or the user decides against it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
