---
id: TASK-26.6.2
title: >-
  Guest crash test: a file whose fsync returned was missing after the crash
  (once, on GCE)
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 09:49'
labels: []
dependencies: []
parent_task_id: TASK-26.6
priority: medium
type: bug
ordinal: 93000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-crash-dax failed once on GCE (host revision 36c382b, x86 guest kernel with 1c6b2cf). After the guest was stopped mid-workload, the host mounted the store, and looking up one of the files whose fsync had returned ("synced W N" lines) failed with ENOENT (vendor/libkrun/tests/test_cases/src/test_fxfs_guest.rs:156). The same test then passed 2 of 2 times on GCE and 20 of 20 in Lima (arm64), with and without 1c6b2cf. So it is rare, and nothing ties it to that change. A durability failure, however rare, breaks doc-2 requirement 2. The failing run artifacts (the crashed store in extents/, and stdout with 546 synced lines) were kept in the session scratchpad: gce-mr1/e2e/fxfs-guest-crash-dax/. They are temporary.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The failure is reproduced (a loop of the crash test on GCE or Lima until it fails) and its cause found
- [ ] #2 A fix, with a test that fails before it, or the cause shown to be in the test itself and the test corrected
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
