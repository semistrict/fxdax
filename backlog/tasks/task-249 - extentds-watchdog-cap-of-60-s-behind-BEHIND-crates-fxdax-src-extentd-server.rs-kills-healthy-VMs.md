---
id: TASK-249
title: >-
  extentd's watchdog cap of 60 s behind (BEHIND,
  crates/fxdax/src/extentd/server.rs) kills healthy VMs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 10:17'
updated_date: '2026-11-10 08:32'
labels:
  - bug
dependencies: []
priority: high
ordinal: 682000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the TASK-136.1 overlay work (thread 7a447b3d, 2026-11-01), never filed: during a DAX clone of the Linux checkout the watchdog let go of a VM in 1 of 4 runs while it was still acknowledging; with the cap lifted the VM peaked 100 s behind and finished. The cap is unchanged on main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The watchdog lets go only of a VM process that is silent, or behind without progress, shown by a test of a slow but acknowledging VM
- [ ] #2 Linux-checkout DAX clone passes 10 of 10 on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Keep the 5 s silence rule. Replace the 60 s cap (BEHIND) with a pace rule: a VM process behind for BEHIND is let go only if, at the pace it acknowledged over the last BEHIND, it would not clear what it is behind on within CATCHING_UP (10 min): a trickle against thousands outstanding takes hours; a slow VM acking thousands a second minutes at most.
2. Acks::look measures the pace per whole window of BEHIND; Silence carries backlog and pace and says them as it lets a VM go.
3. Unit tests: a slow VM acknowledging steadily survives 240 s behind; a stalled VM (one ack every 4 s against 10,000 outstanding) is let go at 60 s; existing tests adjusted to the rule.
4. Linux: cargo nextest on ubu1; linux-checkout-fxfs-dax clone 10 of 10 on ubu1 (GCE retired).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Rule: silent 5 s as before; behind for BEHIND (60 s) is no longer enough. A VM process behind is let go only if, at the pace it acknowledged over the last whole 60 s window, it would not clear its backlog (last_taking - acked) within CATCHING_UP (10 min). A stalled one (an ack every 4 s against 10,000 outstanding: ~11 h at that pace) goes at 60 s as before; one acknowledging hundreds or thousands a second stays however long it is behind. Silence now says backlog and pace as it lets a VM go. Tests (extentd/server.rs acks_tests): slow_vm_processes_acknowledging_are_kept_however_long_behind (300 s behind, 300 acks/s against 100/s more sent), stalled_vm_processes_behind_are_let_go, a_silence_says_which_limit_it_passed updated to the new line.
<!-- SECTION:NOTES:END -->
