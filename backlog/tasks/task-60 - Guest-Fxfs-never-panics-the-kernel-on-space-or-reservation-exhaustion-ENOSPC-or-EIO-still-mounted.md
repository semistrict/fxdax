---
id: TASK-60
title: >-
  Guest Fxfs never panics the kernel on space or reservation exhaustion: ENOSPC
  or EIO, still mounted
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: high
ordinal: 169000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs_rt_panic turns Rust panics in Fxfs (allocator.rs reservation asserts and others) into kernel panics. Space and reservation exhaustion must fail the operation (ENOSPC, or EIO) and keep the filesystem mounted; audit the panics reachable from guest operations.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
