---
id: TASK-258
title: >-
  OpenTelemetry spans in extentd: requests, commits, uploads, fork/open,
  page-in, relief; OTLP export, off by default
status: To Do
assignee: []
created_date: '2026-11-08 14:00'
labels:
  - observability
  - extentd
dependencies: []
ordinal: 691000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Latency inside extentd (a slow MAP, a commit stuck in its durable flush, an upload waiting on the bucket) shows today only as counters in /metrics and lines in the stderr log; nothing ties one request to the store work it caused. Spans exported over OTLP to a collector (Jaeger, otel-collector) let an operator see one request or commit as a tree. The request-ID work (separate task) owns the log format and the wire protocol; this task must not change either, and attaches fxdax.request_id through one named hook until that ID is on main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Spans: each request served, named by kind (MAP, WRITE, COMMIT, READ, FREE, the rest); commit with child spans seal, store write, durable flush; segment uploads and checkpoints with each bucket request a child span; fork/open of a store; page-in of a cached extent; relief/eviction passes
- [ ] #2 Attributes: request kind, store/branch, bytes, extent counts; errors set the span status
- [ ] #3 Off by default; enabled by an extentd option plus the standard OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_SERVICE_NAME, OTEL_TRACES_SAMPLER/_ARG; commits default to parent-based ratio sampling, documented
- [ ] #4 Off-path cost measured and near zero
- [ ] #5 fxdax.request_id attached via one named hook returning Option<id> (the request ID itself is the request-ID task's)
- [ ] #6 Linux test with the in-memory exporter serves a request and a commit through extentd and checks the span tree and attributes; with tracing off nothing is exported
- [ ] #7 docs/ page: pointing extentd at a collector, and the span list
- [ ] #8 Checked end to end against a collector in Docker
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
