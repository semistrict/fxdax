---
id: TASK-150.1
title: Collector deletes objects of inherited extents still uploading
status: To Do
assignee: []
created_date: '2026-11-01 16:49'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 516000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_remote/src/collector.rs:421-427, key at crates/extent_frame/src/segment.rs:218: the in-flight check on an extents/ object uses the branch in its key, which minted the extent, not the branch uploading it. A child with a branch of its own uploads inherited extents of 4 MiB or more as own (remote.rs diff), a pass lists the object before its segment lands, and deletes it after the segment is acknowledged. Reads then fail with Error::Lost once the local copy is gone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
