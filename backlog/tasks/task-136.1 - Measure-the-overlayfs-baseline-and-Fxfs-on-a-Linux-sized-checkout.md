---
id: TASK-136.1
title: Measure Fxfs on a Linux-sized checkout
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 16:04'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: spike
ordinal: 468000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The design makes claims about cost that nothing has measured yet. The numbers decide how much per-guest metadata caching matters. There is no overlayfs comparison: doc-10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The results are recorded in doc-10, with the kernel, host and checkout they came from
- [ ] #2 A repeatable test in the repository measures mount time, first `git status`, first write and memory per sandbox for Fxfs on a Linux-sized checkout, and the same with hundreds of repos mounted at once
- [ ] #3 The test also records the checkout's Fxfs volume layer bytes and allocator records
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
