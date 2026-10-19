---
id: TASK-48
title: 'extentd: gRPC health check on the control API'
status: Done
assignee: []
created_date: '2026-10-19 11:13'
updated_date: '2026-10-19 12:49'
labels: []
dependencies: []
priority: medium
ordinal: 147000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Orchestrators and load balancers probe gRPC services with the standard health protocol (grpc.health.v1.Health). The daemon's control API has none, so whether a daemon is serving can only be inferred from a call that does work.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The control API serves grpc.health.v1.Health: SERVING once the daemon accepts stores, NOT_SERVING while it starts or stops
- [x] #2 Watch streams changes of the status
- [x] #3 A test checks the status through a daemon's start, serving and shutdown
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
tonic-health 0.14.6 (MIT). SERVING while extentd::serve accepts VM processes' connections, for "" and the control service; SIGTERM reports NOT_SERVING, waits until the health service has, and exits 0. Verified: scripts/test.sh on macOS (2096 workspace tests plus the Shuttle suites); Linux on GCE b3 (2152 workspace tests and every suite; guest cases 18/18, fxfs-guest-suspend-dax passing 3/3 on rerun after one vsock harness flake); scripts/upstream-changes.py --check passes. No upstream code changed. Tests: tests/control.rs health_follows_whether_the_daemon_serves (Check and Watch through start, serve, stop); tests/fxdaxctl.rs the_daemon_stops_on_sigterm.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The control socket serves grpc.health.v1.Health, following whether the daemon serves; SIGTERM stops it cleanly after reporting NOT_SERVING. Commit f7f6718.
<!-- SECTION:FINAL_SUMMARY:END -->
