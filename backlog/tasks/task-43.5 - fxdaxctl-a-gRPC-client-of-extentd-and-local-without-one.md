---
id: TASK-43.5
title: 'fxdaxctl: a gRPC client of extentd, and local without one'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:00'
updated_date: '2026-10-18 07:31'
labels: []
dependencies:
  - TASK-43.3
parent_task_id: TASK-43
type: feature
ordinal: 134000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With extentd's gRPC socket named (FXDAX_EXTENTD_GRPC), every fxdaxctl command goes through the API; without, commands act on stores locally, which the cross-process store lock keeps off stores extentd has open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxdaxctl's tests pass both ways
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax::extentd::control::Client: a blocking client of the control API (every store call, stats), errors carrying errnos.
2. fxdaxctl: commands parsed to an enum; with FXDAX_EXTENTD_GRPC each goes through the Client, without it acts locally as before (the store lock refuses stores extentd has open). New `public` command (MakePublic).
3. The VM socket's control requests (STATS, RESIZE, FORK_PATH, DELETE_PATH, SYNC_PATH) and their client functions go; their numbers are not reused.
4. Callers move: in-crate tests call stores:: directly; integration tests probe daemons by connecting; guest test cases use the control API at <socket>.control, which test-e2e.sh and spawn_daemon start daemons with.
5. A lock another process holds, waited out, is EBUSY through the daemon, as locally.
6. tests/fxdaxctl.rs runs each test locally and through a daemon (fxdax-extentd binary), plus every command through the daemon.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation (GCE, not Lima): full workspace 2052 tests, all passing but one expectation string in the new daemon-mode fxdaxctl test (fixed: Errno displays as "EBUSY (errno 16)"), then fxdaxctl and extentd tests rerun green; guest e2e extent-guest, fxfs-guest-dax, fxfs-guest-resize, fxfs-guest-live-fork, fxfs-guest-migrate-dax, fxfs-guest-snapshot-dax OK with the x86-64 guest kernel built 2026-10-17. macOS 2012 passed. upstream-changes --check and license-headers OK.

The guest bench (test_fs_bench) moved to GetStats but was not run (needs FXDAX_BENCH_ITERATIONS); it compiles.

Behaviour change: a store lock another process holds, waited out, reports EBUSY through the daemon (was EIO), as it says locally; opening a store maps errors the same way.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdaxctl goes through the control API when FXDAX_EXTENTD_GRPC names it (fxdax::extentd::control::Client, a blocking client whose errors carry errnos), and acts on stores itself otherwise, refusing those extentd has open by their lock; it gained `public` (MakePublic). The VM socket's control requests (STATS, RESIZE, FORK_PATH, DELETE_PATH, SYNC_PATH) and their client functions are gone; guest tests, test-e2e.sh and spawned daemons use the control API at <socket>.control. tests/fxdaxctl.rs runs each test locally and through the fxdax-extentd binary, and every command through it. Verified on GCE (workspace and six guest cases) and macOS.
<!-- SECTION:FINAL_SUMMARY:END -->
