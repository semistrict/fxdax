---
id: TASK-160.9
title: 'extentd: a typed codec for the wire'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 10:40'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 580000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each request's layout is a doc comment over args[n], encoded by hand in client.rs and decoded by hand in server.rs; stage operations are encoded twice; the daemon never refuses unknown flags (the VMM does); each handler authorizes the paths it names; flow-control bytes are counted on both sides, and the two receivers' size limits differ. doc-11 says nothing checks it. Explains the TASK-79.16 series, 79.43, 150.3, 150.4, 30df0bdff. Proposal: enum Request and StageOp with encode/decode giving today's bytes; decoding refuses unknown flags and extra payload and checks every path and branch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Requests are typed on both ends, the bytes unchanged
- [x] #2 A WRITE with an unknown flag gets EINVAL, red before
- [x] #3 A round-trip property test per kind
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One typed codec for extentd's wire (crates/fxdax/src/extentd/codec.rs): each request, reply and daemon message encoded and decoded in one place, client and server both using it; unknown flags, flags meaning nothing together, payloads a kind doesn't take and malformed arguments refused with EINVAL as decoded. The wire's bytes are unchanged, pinned by a test against the old hand-built headers; a round-trip property test covers all 22 kinds; a WRITE or COMMIT with an unknown flag bit now gets EINVAL and changes nothing (served before). Verified: macOS scripts/test.sh on the tree merged with batch 2026-11-04 (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch, every job (run 41806511048, Linux workspace with the codec's tests); GCE r540 in the jailer under the production seccomp filter (fxfs-guest-dax, checks-dax, crash-dax, migrate-tenant-dax, pgbench pass; the refusal gate's failure is TASK-167's, the jailer never mounting /proc).
<!-- SECTION:FINAL_SUMMARY:END -->
