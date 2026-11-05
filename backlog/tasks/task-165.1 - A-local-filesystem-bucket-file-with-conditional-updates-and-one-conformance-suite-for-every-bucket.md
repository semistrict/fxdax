---
id: TASK-165.1
title: >-
  A local-filesystem bucket: file:// with conditional updates, and one
  conformance suite for every bucket
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:15'
updated_date: '2026-11-05 03:19'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::local: an object_store ObjectStore over LocalFileSystem (fsync on) adding conditional updates and serialised deletes and overwrites, under a bucket-wide lock (a mutex in the process, flock of .fxdax-lock across processes), the lock file kept out of listings.
2. store::object_store_at maps file:///dir to it (made if missing), the throttle as for any store.
3. bucket::tests::conformance: what every bucket must do; run over memory://, file://, the sim's Direct, and in over_a_real_store (RustFS, Azurite).
4. Concurrent updates from one version through two buckets on one directory: one wins.
5. tests/gc.rs over file://. Docs: doc-7 decision 9, README/dev docs naming file://.
<!-- SECTION:PLAN:END -->
