---
id: TASK-136.6
title: 'Attach a repo on path lookup, through the sandbox manager'
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 04:48'
labels: []
dependencies:
  - TASK-136.2
  - TASK-136.4
  - TASK-136.5
documentation:
  - backlog/docs/doc-10 - Fxfs-overlays-for-shared-repository-checkouts.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 473000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A path lookup attaching a repo combines with gyit's path-is-the-URL idea. Since only the sandbox manager attaches, the lookup has to wait on an upcall through a guest agent to it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A lookup under a configured path attaches and mounts the repo, and the lookup returns once it is mounted
- [ ] #2 A repo the sandbox manager refuses fails the lookup with an error, and leaves nothing mounted
- [ ] #3 Concurrent lookups of one repo attach it once
- [ ] #4 An idle repo's mount expires, closing its store and keeping its writable layers, and its next lookup mounts it again
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
