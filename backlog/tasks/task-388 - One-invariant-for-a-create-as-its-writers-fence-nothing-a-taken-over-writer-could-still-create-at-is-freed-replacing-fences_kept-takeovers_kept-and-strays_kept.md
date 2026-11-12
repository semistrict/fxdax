---
id: TASK-388
title: >-
  One invariant for a create as its writer's fence: nothing a taken-over writer
  could still create at is freed, replacing fences_kept, takeovers_kept and
  strays_kept
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 19:26'
updated_date: '2026-11-12 20:07'
labels:
  - extent_store
  - collector
  - correctness
dependencies: []
priority: high
ordinal: 765000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Since TASK-371 a durable commit is one bucket round trip: a writer counts its successful create as its fence and acknowledges without reading its lease. That is safe only while every position a taken-over writer might still create at stays occupied. Three collector rules now keep it so (fences_kept, takeovers_kept from TASK-371; strays_kept from TASK-383), each found only after the simulation lost an acknowledged commit (TASK-377's count, TASK-380's re-read is separate, TASK-383). User decision 2026-11-12: keep the one-round-trip design, but state the safety once instead of by cases. Write the invariant in specs/bucket as a single property (no object at a position some live-or-taken-over writer could still create at is ever deleted, until no such writer can exist) and derive the collector's keep-set from one function that computes those positions, replacing the three rules; doc-7 states the rule once. Then hunt for a fourth hole on purpose: models with more takeovers and writers than today's configs (tractable via Can flags as TASK-380 did), a mutation per way the keep-set could shrink, and a long simulation soak (SIM_SOAK high, the recreated/takeover profiles) on ubu1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 specs/bucket states the invariant once, the collector computes its keep-set from one function, and the three case rules are gone (or are shown to be that function's cases)
- [ ] #2 Models with at least one more takeover and writer than today's configs hold, and a mutation per way of shrinking the keep-set finds a lost acknowledged commit
- [ ] #3 A simulation soak of 100,000 seeds across the takeover profiles loses nothing
- [ ] #4 doc-7 states the rule once, with why it is enough
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. State records facts: fences and strays (void/late) apart; one function collector::pinned(state, lease) computes the keep-set; rules pins_fences, pins_old_fences, pins_void_strays, pins_late_strays, pins_takings are its cases, replacing fences_kept/takeovers_kept/strays_kept. FORMAT 6 -> 7. 2. specs/bucket: one Pinned operator in GcFinish; invariant CreatesNeverFreed (history var freed kept by Next): the first place a live writer (taken over or taking over) may still create at and acknowledge was never freed. 3. Mutation per case (NoPins*), each finding NoAckedLost; hunt models with one takeover and one writer more (BucketTakeovers: 3 hosts, 4 takings). 4. Direct tests per case (rule-off twins), sim test_needs per case. 5. 100,000-seed soak of the takeover profiles on ubu1 (fxjob --mem 8G). 6. doc-7 states the rule once.
<!-- SECTION:PLAN:END -->
