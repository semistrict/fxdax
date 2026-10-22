---
id: TASK-86
title: >-
  PMM: an allocation of an arena at a cap but not starved spins when the node
  says to wait
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: bug
ordinal: 260000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while restructuring the PMM's arenas (integrate-2, review P.2): waits_for_room is true for an arena at a cap even when it isn't starved (it has free pages, or spares would go back), so its allocation skips the node's gate. If the node is in ShouldWaitState::UntilReset, or a random should-wait hits, alloc_page_in returns SHOULD_WAIT; is_starved is false, so nothing waits on room either, and AnonymousPageRequest::allocate loops back at once: a busy spin. Already there at 77d561b; no test reproduces it yet.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the spin (an arena at its cap, not starved, the node in UntilReset)
- [ ] #2 Fixed: such an allocation waits; the zircon and zircon-core-tests suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
