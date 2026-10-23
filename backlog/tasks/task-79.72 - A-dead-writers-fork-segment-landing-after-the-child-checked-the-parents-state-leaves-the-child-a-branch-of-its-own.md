---
id: TASK-79.72
title: >-
  A dead writer's fork segment landing after the child checked the parent's
  state leaves the child a branch of its own
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 323000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The narrow race TASK-79.20's fix leaves (round 1). A dead writer's fork-segment PUT can land in object storage after the child has checked the parent's state without it; the child then gets a branch of its own rather than the fork the parent's footer names.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A child created while its parent's dead writer's fork segment is still in flight either sees the fork or refuses, never a branch of its own
- [ ] #2 A simulation test drives the PUT landing after the child's check
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done. AC#1: the wording allows only 'sees the fork or refuses, never a branch of its own'. When the parent's store is open nowhere (and when it was deleted here), the fix fences the parent's branch so the late PUT can never land, and the child then takes a branch of its own (commit 60595238 says so; the two forks_whose_parents_* tests assert origin None). The evidence proves a different guarantee: the child's own branch never sits beside a fork that lands, since the parent's state is asserted to name no fork. Checking AC#1 needs the user to restate it to match, for example 'never a branch of its own while the fork may still land'. AC#2: there is no simulation test. The late PUT is driven by deterministic Gated object-store tests in fxdax remote.rs, because fxdax's Remote runs on tokio and its own threads, which neither extent_remote's simulation nor Shuttle drives (CrashedFork's doc, round2a report). Closing AC#2 needs either a simulation test of the PUT landing after the child's check, or the user accepting the Gated tests in its place. Audit: No AC is claimed, and the DoD claims hold: all 6 Gated remote tests pass on both platforms, check exits 0, and the touched files are fxdax's own. I agree it is not done. AC#1 as written forbids the child a branch of its own. Per 60595238, a child whose parent is open nowhere, or was deleted here, fences the parent's branch and then takes a branch of its own, and two tests assert this. AC#2 asks for a simulation test. 60595238's message says the late PUT is driven only by Gated object-store tests, because Remote runs on tokio, which neither Shuttle nor extent_remote's simulation drives. Closing either AC needs the user to restate it or accept the substitute.
<!-- SECTION:NOTES:END -->
