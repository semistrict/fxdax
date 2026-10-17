---
id: TASK-28.4
title: 'Object store backend: S3 and GCS through object_store, RustFS in tests'
status: To Do
assignee: []
created_date: '2026-10-17 01:31'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 84000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 decisions 2 and 9. Fencing relies on conditional creates and updates, which some S3-compatible stores lack and which object_store maps per backend (S3 If-None-Match / If-Match, GCS generations), and a create retried after a 5xx can report failure for an object that landed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A backend for S3 and GCS on object_store, configured from a URL and credentials from the environment
- [ ] #2 At startup it probes conditional create and update and refuses a store without them
- [ ] #3 An ambiguous create is resolved by reading the object back, shown by a test with injected faults
- [ ] #4 Workspace tests run against a RustFS server the tests start, and against the in-memory store with fault injection
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
