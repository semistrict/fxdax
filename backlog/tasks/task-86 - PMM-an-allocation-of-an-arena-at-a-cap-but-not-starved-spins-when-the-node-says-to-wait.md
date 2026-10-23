---
id: TASK-86
title: >-
  PMM: an allocation of an arena at a cap but not starved spins when the node
  says to wait
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A test reproduces the spin (an arena at its cap, not starved, the node in UntilReset)
- [x] #2 Fixed: such an allocation waits; the zircon and zircon-core-tests suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Its test arenas_at_a_cap_wait_at_the_gate_while_the_node_delays_allocations was flaky (18 of 2000 runs): the memory watchdog's thread, started with the node, set the free memory signal and lifted the delay the test had set (shown by a probe: every failure had memory-pressure-thread's set_free_memory_signal between the test's set and its wait). Not a code bug. The tests that delay the node now boot with the watchdog's thread never started (184e3353, aa2a97db): 2000 of 2000 pass alone, under load and alongside the suites. The starves() half of the fix has no test: TASK-79.81.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An allocation of an arena at its cap but not starved skipped the node's gate. While the node delayed allocations (UntilReset), it was told SHOULD_WAIT without waiting, so its caller spun. A starved one waited for room that the level's reset never signalled. Now only a starved arena's allocation waits for room, and only while the node does not delay allocations. Every other allocation waits at the gate, as upstream's does (1c97626a). arenas_at_a_cap_wait_at_the_gate_while_the_node_delays_allocations and starved_arenas_wait_at_the_gate_while_the_node_delays_allocations failed before the fix and pass after. They now boot without the memory watchdog's thread, whose own signal setting made the first fail 18 runs in 2000. The starves() half is covered by TASK-79.81's test. The zircon and zircon-core-tests suites pass on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
