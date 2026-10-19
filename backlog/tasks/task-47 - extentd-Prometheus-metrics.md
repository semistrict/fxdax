---
id: TASK-47
title: 'extentd: Prometheus metrics'
status: To Do
assignee: []
created_date: '2026-10-19 11:13'
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
- [ ] #1 extentd serves its metrics in the Prometheus text format on an HTTP endpoint it is told to listen on
- [ ] #2 The metrics cover what GetStats reports, object storage's requests and bytes, stores open and followers, and per-request-kind counts and latency histograms
- [ ] #3 A test scrapes the endpoint of a running daemon and checks the metrics change as requests are served
- [ ] #4 The metrics' names and meanings are documented
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
