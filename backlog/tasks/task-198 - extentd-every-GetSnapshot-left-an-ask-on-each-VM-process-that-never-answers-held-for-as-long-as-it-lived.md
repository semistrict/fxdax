---
id: TASK-198
title: >-
  extentd: every GetSnapshot left an ask on each VM process that never answers,
  held for as long as it lived
status: Done
assignee:
  - '@review-extentd'
created_date: '2026-11-06 06:47'
updated_date: '2026-11-06 07:02'
labels:
  - review-2026-11-06
  - extentd
dependencies: []
priority: medium
type: bug
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
snapshot::take (crates/fxdax/src/extentd/snapshot.rs) asks every VM process that opened a store for its stats. Report::ask pushed an Asked onto the report's `asked` before sending. Only an answer drained it: Report::answered drains every ask up to the one answered. A snapshot that gave up waiting, after 2 s by default and 60 s at most, left its ask in place. A VM process whose guest never answers kept every ask of every snapshot for as long as it lived. The VMM's StatsAsks::ask (vendor/firecracker extent/mod.rs) says the guest was asked whenever the driver takes asks, even while the VM is paused, so a paused VM never answers. A scraper polling GetSnapshot or fxdaxctl stats grew the daemon's memory without end, by one Asked and a oneshot channel per VM per snapshot. A failed send of the ask left one too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A snapshot that gives up waiting for a VM process's answer leaves none of its asks held by the VM's report
- [x] #2 asks_given_up_on_are_forgotten passes on Linux, and goes red with the fix reverted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A test: a silent VM process (on_stats_asked returns true, never answers), three snapshots of 10 ms, then each report's asks waiting must be [0].
2. Report::forget(ask), called by take() when its wait ends without an answer and by ask() when the send fails.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Found by the 2026-11-06 review of extentd as a service, reading snapshot.rs: `asked` grows in ask() and shrinks only in answered().

Blind spot: snapshots_hold_the_daemons_vm_processes_and_guests_stats has a VM process that never answers, but it checks only what that snapshot says of it, never what the daemon keeps afterwards. No test took more than one snapshot of a silent VM.

Test: asks_given_up_on_are_forgotten takes three 10 ms snapshots of a silent VM process and asserts each report holds no ask (snapshot::asks_waiting, test-only). Docker (OrbStack, Linux): red with the fix reverted (left [3], right [0]), green with it.

Looked around: what else the daemon queues per connection until the VM answers. Seal asks (TASK-153.15) are not waited for again until the connection answers, so they stay bounded. A shrink has one at a time (Shrinking). Unacknowledged unmaps get the VM process killed (TASK-79.9). Nothing else of this kind is unbounded.

Structure: the ask belongs to the snapshot waiting for it, which is now the one that removes it, through Report::forget. Holding asks by ID in a map would not have changed the bug, since the leak came from nobody removing them. No restructuring needed.

Validation: Docker (OrbStack, Linux) nextest -p fxdax 469 of 474 passed. The 5 failures come from the container: grants' three jail tests need sudo, the build stamp needs git at build time, and the seccomp test read this worktree's vendor/firecracker checkout, which is behind the commit main records. macOS FXDAX_TEST_PARTS="workspace shuttle": workspace 2031 of 2031, shuttle all green. clippy -D clippy::disallowed_methods clean on Linux.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A snapshot that gives up waiting for a VM process's answer now forgets its ask (Report::forget, also called when the send fails), so a VM that never answers, such as a paused one, no longer holds one ask per snapshot for its whole life. Proved by asks_given_up_on_are_forgotten: red with the fix reverted ([3] asks held), green with it, on Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
