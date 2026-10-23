---
id: TASK-79.81
title: 'zircon: no test covers the starves() half of TASK-86''s fix'
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:47'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: task
ordinal: 340000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing the fix of TASK-86's flaky test (round 2a). TASK-86's fix has two halves in crates/zircon/src/vm/pmm.rs: waits_for_room, which both TASK-86 tests pin (reverting it fails them 200 of 200), and the UntilReset check in starves(), which none does: reverting it alone, all zircon tests pass. That half covers a starved arena's allocation passing the gate just before the node trips to UntilReset: it would wait for room the level reset never signals, and relief and eviction are never asked. The window cannot be reached from one thread (Event::wait and EventWait::poll re-check under the event's lock, and the async wait runs the gate, allocate_after_wait and starves() in one poll); it needs another thread tripping the level between the gate's wait returning and alloc_page_in's delay check, or after allocate_after_wait reads should_wait. Shuttle shows nothing there because no relief thread runs under it. A test needs a seam.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test that fails with starves()'s UntilReset check reverted and passes with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
No test covered the starves() half of TASK-86's fix: an allocation of an arena starved at its cap that passes the node's gate just before the node trips to UntilReset. A test-only seam, pmm::past_the_gate, now runs a closure just past the gate, where another thread might trip the level. allocations_delayed_past_the_gate_wait_at_it_again uses it to show that such an allocation, sync and async, is told to wait and then waits at the shut gate, not for room that never comes. With the UntilReset check reverted the test fails ('it waited for room'). With it, the test passes 200 of 200 and in the macOS and Linux workspace runs.
<!-- SECTION:FINAL_SUMMARY:END -->
