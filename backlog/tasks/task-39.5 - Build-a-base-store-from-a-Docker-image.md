---
id: TASK-39.5
title: Build a base store from a Docker image
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 115000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
There is no supported way to turn a Docker (OCI) image into a base store. A tool (fxdaxctl) should take an image (or its exported root filesystem) and write an Fxfs filesystem into a new store, with owners, modes, xattrs, hard links, symlinks and devices as the image has them, ready to fork.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxdaxctl builds a base store from an OCI image (or a docker export tarball), shown by a test that boots a guest on a fork of it and checks files, owners, modes, links and xattrs
- [ ] #2 Building is reproducible enough to compare: the same image gives the same files
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
