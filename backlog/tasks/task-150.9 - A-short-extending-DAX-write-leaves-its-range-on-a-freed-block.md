---
id: TASK-150.9
title: A short extending DAX write leaves its range on a freed block
status: To Do
assignee: []
created_date: '2026-11-01 16:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 524000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. dax.c:3533 omits FXFS_SET_KEEP_TAIL; crates/fxfs-posix/src/lib.rs:1662 calls zero_tail, moving the last block, while the window keeps mapping the old one writably. A later append with no seal goes to the freed block, and acknowledged bytes read as zeros once remapped. test_append_stopped_short misses it, its size being page-aligned.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
