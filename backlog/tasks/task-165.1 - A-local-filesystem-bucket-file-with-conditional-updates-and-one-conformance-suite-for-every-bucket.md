---
id: TASK-165.1
title: >-
  A local-filesystem bucket: file:// with conditional updates, and one
  conformance suite for every bucket
status: To Do
assignee: []
created_date: '2026-11-05 03:15'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 623000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 1. Development, tests and images need a bucket on the host's filesystem; object_store's LocalFileSystem refuses PutMode::Update, so a `file://` URL parses but every lease and record update retries forever and `probe` fails. Additive: nothing changes for memory://, s3://, gs://, az://. The conformance suite also pins down what every bucket must do, which today only the simulation and RustFS cases imply.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 `Store::from_url("file:///dir/prefix")` gives a bucket that passes `store::probe`: a create where an object is fails, an update of another version fails, an update of the object's version succeeds
- [ ] #2 One conformance suite (create-only, conditional update and delete, ranged and suffix reads, sorted listing by prefix, missing objects) runs against memory://, file:// and the simulation's Direct, and against RustFS and Azurite when their URLs are set
- [ ] #3 Two processes updating one object of a file:// bucket from the same version: exactly one succeeds (a test with two processes or two buckets on one directory)
- [ ] #4 `fxdax-gc file:///...` collects a file:// bucket (tests/gc.rs)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
