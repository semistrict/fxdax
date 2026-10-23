---
id: TASK-79.9
title: >-
  One unresponsive VM process wedges reclamation and faults for every tenant:
  stage unmaps wait for its ACK while holding VMO and MMU locks
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:10'
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
