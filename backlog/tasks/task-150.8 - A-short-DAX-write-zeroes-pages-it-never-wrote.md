---
id: TASK-150.8
title: A short DAX write zeroes pages it never wrote
status: To Do
assignee: []
created_date: '2026-11-01 16:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 523000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. dax.c:2800 and :3284 pass keep = false; crates/fxfs/src/object_store/data_object_handle.rs:2148-2161 copies nothing for whole pages. Blocks a write fully covers move to new zeroed blocks before the copy, so a source faulting partway leaves zeros where old data was, and a source mapping the same page copies zeros over itself and reports success. test_short_copy is excluded on DAX.
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
