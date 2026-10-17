---
id: TASK-28.4
title: 'Object store backend: S3 and GCS through object_store, RustFS in tests'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 03:54'
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
- [x] #1 A backend for S3 and GCS on object_store, configured from a URL and credentials from the environment
- [x] #2 At startup it probes conditional create and update and refuses a store without them
- [x] #3 An ambiguous create is resolved by reading the object back, shown by a test with injected faults
- [x] #4 Workspace tests run against a RustFS server the tests start, and against the in-memory store with fault injection
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. bucket::Version carries both an ETag (S3) and a version (GCS generation).
2. extent_remote::store::Store: Bucket over any object_store ObjectStore under a prefix; from_url (s3://, gs://, memory://) with credentials from the environment; errors mapped to NotFound/AlreadyExists/Precondition, the rest retried as Unavailable (decision 3: wait).
3. probe(): create twice, update of another version, update of the version; refuse otherwise.
4. Tests: object_store InMemory (keys under the prefix, probe, a branch lifecycle with fork, frees, takeover, collection); ambiguous creates against the simulation bucket; the lifecycle against RustFS via scripts/test-rustfs.sh (starts RustFS, makes a bucket with the AWS CLI, runs the ignored rustfs test).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
RustFS (built from ~/src/rustfs at 1.0.2, cargo build --release --bin rustfs, 30 min) honours If-None-Match and If-Match: scripts/test-rustfs.sh passes (probe + branch lifecycle). Deviation from AC #4: the RustFS test is ignored in the workspace run, as it needs the RustFS binary and the AWS CLI; scripts/test-rustfs.sh runs it. The in-memory store and simulation tests run in the workspace.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extent_remote::store::Store implements Bucket over object_store (S3, GCS, memory), from a URL with credentials from the environment; probe() refuses stores without conditional writes. Verified: in-memory tests (prefix, probe, branch lifecycle), ambiguous creates against the simulation bucket, and scripts/test-rustfs.sh against RustFS 1.0.2 (probe + lifecycle pass). macOS 1910, Linux 1916 passed.
<!-- SECTION:FINAL_SUMMARY:END -->
