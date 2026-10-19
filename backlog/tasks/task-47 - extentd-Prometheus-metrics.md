---
id: TASK-47
title: 'extentd: Prometheus metrics'
status: Done
assignee: []
created_date: '2026-10-19 11:13'
updated_date: '2026-10-19 12:49'
labels: []
dependencies: []
priority: medium
ordinal: 146000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The daemon's counters (requests and their latencies, object storage requests and bytes, the extent cache, stores open, followers) are reachable only through the control API's GetStats and the stats file, which no monitoring system scrapes. Operators need them in Prometheus.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extentd serves its metrics in the Prometheus text format on an HTTP endpoint it is told to listen on
- [x] #2 The metrics cover what GetStats reports, object storage's requests and bytes, stores open and followers, and per-request-kind counts and latency histograms
- [x] #3 A test scrapes the endpoint of a running daemon and checks the metrics change as requests are served
- [x] #4 The metrics' names and meanings are documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
No new dependency: a minimal HTTP/1.1 responder (GET /metrics, else 404) on the TCP address FXDAX_EXTENTD_METRICS names. Per-kind counts, errors and a 10us..1s histogram, counted before each reply; gauges connections, stores_open, follower_views; GetStats counters as fxdax_extentd_<name>_total; bucket requests and bytes. Names documented in extentd::metrics' module docs. Verified: scripts/test.sh on macOS (2096 workspace tests plus the Shuttle suites); Linux on GCE b3 (2152 workspace tests and every suite; guest cases 18/18, fxfs-guest-suspend-dax passing 3/3 on rerun after one vsock harness flake); scripts/upstream-changes.py --check passes. No upstream code changed. Test: extentd/tests.rs metrics_count_requests_as_they_are_served.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd serves Prometheus metrics over HTTP when FXDAX_EXTENTD_METRICS is set: requests by kind with errors and latency histograms, connections, stores open, followers, GetStats counters and object storage. Commit f7f6718.
<!-- SECTION:FINAL_SUMMARY:END -->
