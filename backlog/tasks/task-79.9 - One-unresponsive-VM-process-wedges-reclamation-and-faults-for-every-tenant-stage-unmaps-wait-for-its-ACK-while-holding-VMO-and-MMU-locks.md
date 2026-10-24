---
id: TASK-79.9
title: >-
  One unresponsive VM process wedges reclamation and faults for every tenant:
  stage unmaps wait for its ACK while holding VMO and MMU locks
status: In Progress
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 197000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
RemoteStage::unmap waits with no deadline for the VM process's ACK, and it is called under the VMO's cow lock and the window's MMU lock. A frozen or hostile VMM that maps a shared public-base extent blocks the single relief thread and every other tenant's faults on that extent. Allocations at their cap then stall until AnonymousPageRequest panics after 30 s and extentd dies.

Where: crates/fxdax/src/extentd/server.rs:945, crates/fxdax/src/extentd/server.rs:978, crates/fxdax/src/extentd/server.rs:1039, crates/zircon/src/arch/host/mmu.rs:416, crates/zircon/src/vm/vm_cow_pages.rs:3555, crates/zircon/src/vm/vm_mapping.rs:708, crates/fxdax/src/extents.rs:166, crates/zircon/src/vm/anonymous_page_request.rs:118.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.17, TASK-50.3, TASK-56. No task covers stage unmaps waiting with no deadline for a VM process's ACK under VMO and MMU locks. TASK-26.17 designed the acknowledged unmap and protect. TASK-50.3 (Done) added the relief thread this stalls. TASK-56 (To Do) is about the VMM's reply-receiving thread, which also applies stage ops.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): fixed on branch review/r2b-extentd (48 commits on main b5eb2fc8, rebased onto 709a7017), not on main. Verified on Linux (GCE): workspace 2614/2614 and every Shuttle suite; guest fxfs-guest, -dax, -checks, extent-guest, -follow, -live-fork, -daemon-lost, -snapshot, -snapshot-dax, -pgbench, -pgbench-dax OK. fxfs-guest-checks-dax failed on TASK-79.104's guest crash (fixed since on main), and fxfs-guest-follow-torture failed 1 of 3 on main alone and 1 of 3 with these commits (TASK-79.58's flake). Blocked only on a push: the branch's vendor/firecracker points at 6610d88f1, three commits on semistrict/firecracker fxdax-v1.16.1 past 1fbc852f9 (535defc6e TASK-79.41, 8ea641a6d and 6610d88f1 TASK-92.13), in /Users/ramon/src/fxdax/.claude/worktrees/r2bm/vendor/firecracker; push those first, then the branch to main, rebasing onto main (guest-only changes since).

On main (2026-10-24): round 2b rebased onto main and pushed with the guest batch, 08790a954..b158ce82b; Firecracker 535defc6e..6610d88f1 on fxdax-v1.16.1. Green on macOS (scripts/test.sh, 2508) and Linux (GCE r65: workspace 2620, Firecracker extent tests, 17 guest cases). The criterion of a test failing before the fix isn't recorded here: check the round 2b commit that names this task before ticking it.
<!-- SECTION:NOTES:END -->
