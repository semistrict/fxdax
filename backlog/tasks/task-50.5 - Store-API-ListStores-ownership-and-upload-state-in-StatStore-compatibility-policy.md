---
id: TASK-50.5
title: >-
  Store API: ListStores, ownership and upload state in StatStore, compatibility
  policy
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 11:54'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: medium
ordinal: 154000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
sandbox-host rebuilds its view of a host's stores after it restarts, and billing reads every store; neither should take a call per store. Fencing needs to know which placement holds a store, and stop deadlines and alerts need what is not yet uploaded. The gRPC definitions have no compatibility promise.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 ListStores returns every store the daemon serves, with StatStore's fields
- [x] #2 StatStore reports the lease's token and when it was taken, the latest uploaded commit, and the bytes pending upload and the age of the oldest
- [x] #3 A compatibility policy for the control API's protobuf definitions is documented
- [x] #4 Tests cover each new field and call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
ListStores (stores open, writers and followers, each with StatStore's fields); StatStore's new fields 10-16: uploading, lease token, epoch and when taken, latest epoch uploaded, bytes pending, oldest pending's age (Remote::status, kept by the uploader); v1's compatibility policy in the proto. Tests: tests/control.rs open_stores_are_listed_with_their_leases_and_uploads and the closed store's stat. Commit 67f8d36; workspace suites pass on macOS and Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
