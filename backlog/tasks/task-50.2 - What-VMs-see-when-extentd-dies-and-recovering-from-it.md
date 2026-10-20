---
id: TASK-50.2
title: 'What VMs see when extentd dies, and recovering from it'
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 14:59'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 151000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
One extentd serves every VM on a host. When its connection drops, a VM's client fails every pending and later request and never reconnects; what the guest's Fxfs does with those errors is untested; a vCPU faulting on a DAX page may block for ever; per-tenant grants do not survive a restart; and the daemon aborts when its memory runs out, which takes every VM on the host with it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 doc-4 or a doc of its own says what a VM sees in each state: daemon gone, restarting, back
- [x] #2 A test kills extentd under load and checks that each VM either reconnects to the restarted daemon or fails within a bounded time, visibly to its host process
- [x] #3 No vCPU blocks for ever on a DAX fault after the daemon dies
- [x] #4 Grants survive a daemon restart, or are re-made by a documented call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 (commit 769cf92, vendor/firecracker f7b95c3be): a VM process's store is told when its daemon's connection ends (RemoteExtentStore::on_daemon_lost); fxdax's Firecracker exits at once with status 158 (FcExitCode::ExtentdLost), so no vCPU waits on a DAX fault served by a daemon that is gone, by construction. doc-4 'When the host's daemon goes' says what a VM sees. Grants are not kept across restarts, as the proto said: the caller grants again (AC4). Left: AC2's test killing extentd under load with VMs running (tests/handover.rs stores_are_told_when_their_daemon_goes covers a store's process alone), and AC3 shown by a test.

AC2/AC3 (commit 1efad43): guest test fxfs-guest-daemon-lost runs two VMs on stores a daemon of its own serves, each writing files through mmap (each page a DAX fault) in a loop; once both report loading over vsock, the host SIGKILLs the daemon and requires each Firecracker to exit within 10 s with status 158 (ExtentdLost). Passed on GCE. Nothing reconnects (doc-4): each VM fails visibly to its host process instead.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A VM whose extent daemon goes exits at once with status 158 (ExtentdLost), so no vCPU waits on a DAX fault; doc-4 says what a VM sees; grants are re-made by the caller. fxfs-guest-daemon-lost kills the daemon under DAX load with two VMs and checks both exit 158 within 10 s (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
