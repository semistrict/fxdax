---
id: TASK-165.2
title: >-
  TLA+ of doc-7 with the host's WAL: leases, fences, segments, forks, points,
  the collector; a mutation per rule
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 624000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 2; TASK-165 AC #3; covers TASK-44.5 AC #1. The simulation samples interleavings and missed TASK-79.18 and 79.3 until after the fact; TLC exhausts small cases. The restructure adds protocol (a WAL rebased after a takeover), so the spec comes before that code: a design bug is cheapest found here.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 specs/bucket/Bucket.tla models leases with fencing tokens, the fence, dense create-only segments linked by after, acknowledgement after a lease read, hosts crashing with or without their WAL and rebasing it after the fence, forks recorded after acknowledgement, points, and a collector that lists first
- [ ] #2 Invariants hold at small bounds (2 hosts, 2 branches, 4 segments, 1 collector): no acknowledged commit lost, each history a path of its own commits, no ID minted twice, nothing a live branch needs deleted
- [ ] #3 One mutation per field of extent_remote's Rules, plus wal_follows and wal_after_fence, each giving a counterexample, run by make check-tla (scripts/check-tla.sh)
- [ ] #4 doc-7 and doc-14 name the spec, and each rule names its mutation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
