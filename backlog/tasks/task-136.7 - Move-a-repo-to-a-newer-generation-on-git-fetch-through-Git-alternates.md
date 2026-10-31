---
id: TASK-136.7
title: 'Move a repo to a newer generation on git fetch, through Git alternates'
status: To Do
assignee: []
created_date: '2026-10-31 04:49'
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
ordinal: 474000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A mounted worktree's base doesn't change, but `git fetch` in a sandbox should not download what infrastructure already published. Ramon chose the Git level first: objects are content-addressed, so reaching a newer generation's objects through alternates cannot conflict with anything the sandbox wrote, and needs no Fxfs change.

Git has no pre-fetch hook (from memory, unchecked), so a remote helper installed through `url.insteadOf` runs first. As only the sandbox manager attaches, it asks through the guest agent. An in-place swap of base layers stays possible later: doc-10 records what it would need.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 With a newer generation published, `git fetch` in a sandbox transfers no object that generation holds, shown by a test counting what the remote sent
- [ ] #2 A fast-forward `git pull` after it writes only the changed files into the writable layers, and the next `git status` hashes only those
- [ ] #3 Local commits, staged and unstaged changes and untracked files survive the move, and objects of the earlier generation stay readable
- [ ] #4 A sandbox manager refusing the attach leaves an ordinary fetch, which succeeds
- [ ] #5 The read-only generation and its alternates path keep working after snapshot, restore and reopening a stopped store
- [ ] #6 doc-10 records the writable layers' growth across repeated pulls on a Linux-sized checkout
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
