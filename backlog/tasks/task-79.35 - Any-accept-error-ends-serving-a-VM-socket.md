---
id: TASK-79.35
title: Any accept error ends serving a VM socket
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
priority: low
type: bug
ordinal: 223000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
serve_as returns on the first accept error. On the default socket the daemon exits and every VM stops; on a tenant socket the listener stays bound but nothing accepts on it, so new VMs hang on OPEN.

Where: crates/fxdax/src/extentd/server.rs:64, crates/fxdax/src/bin/fxdax-extentd.rs:115, crates/fxdax/src/extentd/grants.rs:76, crates/fxdax/src/extentd/grants.rs:57.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-43.4, TASK-50.2, TASK-34. No task covers serve_as returning on an accept error. TASK-43.4 built the per-tenant listeners (serve_tenant). TASK-50.2 covers VMs when extentd dies. TASK-34 raised the open-files limit.
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
