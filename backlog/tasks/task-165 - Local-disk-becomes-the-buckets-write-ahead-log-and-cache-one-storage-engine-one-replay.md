---
id: TASK-165
title: >-
  Local disk becomes the bucket's write-ahead log and cache: one storage engine,
  one replay
status: To Do
assignee: []
created_date: '2026-11-04 07:34'
labels: []
dependencies:
  - TASK-164
priority: high
type: task
ordinal: 610000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decided by the user on 2026-11-04 (docs/reviews/2026-11-04-first-principles.md, section 2), to start after the read-only DAX work. extent_store's local lineage (records, layers, ledger, index, cleaning, rebuild) and extent_remote each hold every branch, crates/fxdax/src/remote.rs keeping them agreeing; 13+ bugs at the seam, three footer replays (160.15), two cleaners, fork bugs fixed three times. New design: local disk holds only unacknowledged closed segments and a part cache; forks, points, records, checkpoints and cleaning exist only in the bucket protocol; one replay; a local-filesystem Bucket for development. Then a serial commit log over parallel data (a SlateDB-style manifest) and a TLA+ spec of doc-7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A design doc, with what goes and the format change
- [ ] #2 One engine, one replay, the workspace and guest suites passing, the seam's tests kept as tests of the bucket protocol
- [ ] #3 A TLA+ spec of the doc-7 protocol with mutations
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
