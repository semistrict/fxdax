---
id: TASK-136.4
title: >-
  Publish base generations with a manifest, and show a fork keeps .git/index
  valid
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 04:48'
labels: []
dependencies:
  - TASK-136.3
documentation:
  - backlog/docs/doc-10 - Fxfs-overlays-for-shared-repository-checkouts.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 471000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Infrastructure keeps a base with a full checkout current. Attach needs a generation that stays put and a small description of it, so that no guest replays a base's journal. A fork is also only worth having if its first `git status` trusts the index: object IDs and stored times should carry over, but uid and racily clean entries can force a rehash.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A publisher updates a repo in a base store and publishes a generation and its manifest, with earlier generations still attachable
- [ ] #2 A test shows the first `git status` in a fork lstats every tracked file and hashes none, as the sandbox user
- [ ] #3 What a publisher must do for that to hold (uid, index refresh after racy timestamps) is written in doc-10
- [ ] #4 A generation is a fork alone, with no `snapshot_volume`, and its volume is flushed first, so that its manifest leaves nothing in the journal
- [ ] #5 Each generation holds every object of the earlier ones, unreachable ones included
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
