---
id: TASK-249
title: >-
  extentd's watchdog cap of 60 s behind (BEHIND,
  crates/fxdax/src/extentd/server.rs) kills healthy VMs
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 10:17'
updated_date: '2026-11-10 10:20'
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
- [x] #1 The watchdog lets go only of a VM process that is silent, or behind without progress, shown by a test of a slow but acknowledging VM
- [x] #2 Linux-checkout DAX clone passes 10 of 10 on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

Linux: cargo nextest --workspace on ubu1, 2452 of 2457 passed; the 5 failing fail the same on main f36453323 in its own worktree (TASK-315, TASK-316: ZFS, a near-full tmpfs /tmp, the OOM killer; two of them flaky under ubu1's load, passing on rerun). The new acks tests pass there. AC #2 on ubu1, not GCE (retired): linux-checkout-fxfs-dax, FXDAX_E2E_ROUNDS=10, on origin/main 29efab56 (with these commits), guest kernel t310's vmlinux (guest code differs from main only by TASK-311's panic-on-oops config), the case's daemon capped at 2 GiB (FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY) for ubu1's memory: 10 rounds passed, 202-392 s each, no VM process let go in any log. Main CI 42791361922 green with these commits.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's watchdog no longer lets go of a VM process for being 60 s behind: silent 5 s still goes, and one behind for 60 s goes only if its pace over the last 60 s would not clear its backlog within 10 min (CATCHING_UP), so a stalled one acking a trickle still goes at 60 s while a slow one acking steadily is kept however long it is behind. Its log line now says backlog and pace. Verified by new acks tests (a slow VM kept 300 s behind; a stalled one let go at 60 s), macOS workspace 2206/2206, Linux workspace on ubu1 (only environment failures, same on main), linux-checkout-fxfs-dax 10/10 on ubu1, main CI 42791361922.
<!-- SECTION:FINAL_SUMMARY:END -->
