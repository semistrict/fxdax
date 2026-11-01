---
id: TASK-150.6
title: Freed extent IDs can be minted again after a take
status: To Do
assignee: []
created_date: '2026-11-01 16:50'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 521000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_store/src/store.rs:1171 (adopt), :810-820 (check_opens): Spent lives in the local index, so a taken store starts with it empty, and sequence_floor is advisory. A guest can write an ID it sealed and freed, breaking written-once IDs, and a fork still holding the old extent shares its cache entry.
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
