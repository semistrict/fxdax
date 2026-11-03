---
id: TASK-150.3
title: A window size voids the connection memory bound
status: Done
assignee: []
created_date: '2026-11-01 16:50'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 518000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1071, :1718-1721, :803; crates/fxdax/src/dax.rs:472-476: Peer::room() grows with a window size the VM process chooses, uncapped. A WINDOW of 1<<60 then 64 MiB WRITEs faster than served hold up to MAX_REQUESTS x 64 MiB in the daemon, which serves every tenant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: vm_processes_naming_vast_windows_are_let_go_all_the_same. Fixed: MAX_WINDOW, 64 GiB, EINVAL past it, Peer::set_window asserts it. Blind spot: the fuzz used one window size. Class test: the fuzz names any size, red with the bug back. Around it: TRANSFER, READ, FREE_MANY capped; OPEN capacity uncapped but allocates nothing. Structure fine: one gate for one number. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Confirmed and fixed (6a739453b, 33020154e): windows are capped at 64 GiB (MAX_WINDOW, EINVAL beyond), Peer::set_window asserts it. vm_processes_naming_vast_windows_are_let_go_all_the_same, and the fuzz whatever_a_vm_process_says_harms_no_other now naming any window size, went red with the bug back.
<!-- SECTION:FINAL_SUMMARY:END -->
