---
id: TASK-79.17
title: The default tenant's pages share the public arena that every tenant maps
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 205000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Stores of connections with no tenant allocate in ArenaId::DEFAULT, which holds public bases' pages. Every tenant gets that arena read-only and default-tenant VMMs get it read-write, so tenants can read default-tenant data and default-tenant VMMs can write public bases' pages.

Where: crates/fxdax/src/extentd/server.rs:351, crates/fxdax/src/extentd/server.rs:617, crates/fxdax/src/extentd/server.rs:92, crates/fxdax/src/extentd/server.rs:714.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-43.2, TASK-43.7, TASK-39.2. Not tracked. TASK-43.2 (Done) put public bases in the default arena. TASK-43.7 (Done) made the default socket the default tenant. Together they produce this sharing.
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
