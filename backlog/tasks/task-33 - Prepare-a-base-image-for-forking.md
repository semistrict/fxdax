---
id: TASK-33
title: Prepare a base image for forking
status: To Do
assignee: []
created_date: '2026-10-17 15:26'
labels:
  - performance
  - forks
dependencies: []
priority: medium
ordinal: 102000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Children forked from a base image mount whatever state the base was left in: Fxfs's LSM layers as they are, possibly a journal to replay, the store's segment open. Each of a few hundred children then pays the same replay and reads the same uncompacted layers at mount. A step that readies an image once, before it is forked many times, makes each child's mount cheaper. Scope: the disk image (the guest's Fxfs on the store, extent_store, fxdax-branch or extentd tooling).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A command prepares a store as a base image: its Fxfs compacted to as few layers as it allows, its journal flushed so that a mount replays nothing, its open segment closed
- [ ] #2 A fork of a prepared base mounts with no journal replay and reads fewer bytes at mount than a fork of the same base unprepared, shown by a test or the fs bench
- [ ] #3 Preparing an image that is in use by a VM is refused, or waits for it to stop
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
