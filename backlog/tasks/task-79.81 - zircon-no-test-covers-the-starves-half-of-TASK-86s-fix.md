---
id: TASK-79.81
title: 'zircon: no test covers the starves() half of TASK-86''s fix'
status: To Do
assignee: []
created_date: '2026-10-23 06:47'
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
- [ ] #1 A test that fails with starves()'s UntilReset check reverted and passes with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
