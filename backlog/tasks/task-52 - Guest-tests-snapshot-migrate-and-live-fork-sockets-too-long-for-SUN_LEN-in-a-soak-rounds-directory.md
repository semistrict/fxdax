---
id: TASK-52
title: >-
  Guest tests: snapshot, migrate and live-fork sockets too long for SUN_LEN in a
  soak round's directory
status: Done
assignee: []
created_date: '2026-10-20 12:30'
updated_date: '2026-10-20 14:27'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 160000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Under FXDAX_E2E_ROUNDS a case's directory is results/<case>/round-N/<case>, and fxfs-guest-snapshot-dax's, -migrate-dax's and -live-fork's sockets in it (Firecracker's API sockets, the migrate test's own daemon's) exceed 108 bytes: Firecracker fails 'path must be shorter than SUN_LEN', and the test 'never came up'. Fix: those sockets in the temporary directory, as Vm::api_socket and the follow tests' are.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cases pass under FXDAX_E2E_ROUNDS on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sockets in the temporary directory (e4890ce); the cases pass 30 rounds each under FXDAX_E2E_ROUNDS on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
