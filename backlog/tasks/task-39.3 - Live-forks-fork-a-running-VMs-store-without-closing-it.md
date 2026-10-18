---
id: TASK-39.3
title: 'Live forks: fork a running VM''s store without closing it'
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 00:04'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 113000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Forking needs its source closed (fxdaxctl fork); the daemon's FORK request forks an open store but seals the guest's open extents under it. Snapshotting a running box needs a fork at a point the guest's filesystem agrees to: quiesce it (freeze, as fsfreeze does), commit, fork, thaw, while the VM keeps running.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A running VM's store is forked at a consistent point without stopping the VM, the guest's writes after it unaffected, shown by a guest test
- [x] #2 The fork opens in another VM with the filesystem as of the fork point, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentStore::fork_quiescent: a fork only once nothing is open, under a lock that writes and writable mappings take shared, so none slips between the check and the fork (whose commit would seal it under its writer). 2. extentd FORK_PATH: forks a store by path from a connection of its own: live through the holder's store if a VM has it open (fork_quiescent, EBUSY unless quiescent), else opened here; authorized as TASK-39.2. extentd::fork, and fxdaxctl fork through the daemon with FXDAX_EXTENTD. 3. The guest's part is TASK-39.1's freeze: fsfreeze, fork, thaw. 4. Tests: extentd stores_fork_live_once_nothing_is_open; guest fxfs-guest-live-fork.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as planned. The snapshot path's FORK_QUIESCENT uses fork_quiescent too.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A running VM's store forks live through the extent daemon (FORK_PATH; extentd::fork; fxdaxctl fork with FXDAX_EXTENTD) once nothing is open on it, as with its guest's Fxfs frozen (fsfreeze, then thaw): ExtentStore::fork_quiescent checks and forks under a lock that writes and writable mappings take shared, so nothing is sealed under the guest, which goes on writing. Verified: extentd stores_fork_live_once_nothing_is_open (EBUSY while open, fork as of the commit, the parent unaffected); guest fxfs-guest-live-fork (the source writes after the fork and remounts fine; a second VM boots on the fork with the filesystem as of the fork point and writes its own); Linux 1998 / macOS 1976 workspace tests; the 14 guest suites in Lima.
<!-- SECTION:FINAL_SUMMARY:END -->
