---
id: TASK-165
title: >-
  Local disk becomes the bucket's write-ahead log and cache: one storage engine,
  one replay
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-04 07:34'
updated_date: '2026-11-06 02:22'
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
- [x] #1 A design doc, with what goes and the format change
- [ ] #2 One engine, one replay, the workspace and guest suites passing, the seam's tests kept as tests of the bucket protocol
- [x] #3 A TLA+ spec of the doc-7 protocol with mutations
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Design doc (doc-14): what goes, local disk afterwards, formats, the one replay, the file bucket, tests, steps.
2. Subtasks TASK-165.1 to 165.10, one per step of doc-14, with acceptance criteria and dependencies.
3. Decisions in OPEN_QUESTIONS.md.
4. Step 1 (TASK-165.1, the file:// bucket) on this branch, least risk, additive.
The rest per the subtasks, after TASK-164 and TASK-160.8 for steps 7 to 9.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: doc-14 (2026-11-04). Inputs: review section 2, doc-7, the code of extent_store, extent_remote, remote.rs, extents.rs's fork path. Seam bugs classified into four classes the design removes (doc-14, Why). Steps: TASK-165.1 to 165.10. Decisions: OPEN_QUESTIONS.md, "Local disk as the bucket's WAL and cache: how".

2026-11-05: TASK-165.3, 165.4, 165.5 and 165.6 done on main (GCE r691 and r690, CI 42137709782, macOS workspace 2118 of 2118 at 752515020). AC #3 met: specs/bucket's Bucket.tla (TASK-165.2, done) models doc-7 with the WAL; check-tla.sh 11 of 11 models hold and 32 of 32 mutations give their counterexamples (TASK-165.5's run after its rebase), 43 of 43 at 752515020's triage. AC #2 open: TASK-165.7 to 165.10 are To Do (the local lineage, cleaning and index still there).
<!-- SECTION:NOTES:END -->
