---
id: TASK-150.13
title: Failed MAPs after a move can leave a range stale
status: Done
assignee: []
created_date: '2026-11-01 16:52'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 528000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. dax.c:3386 leaves fxfs_dax_map_moved errors raw, not -ESTALE; :3624 and :3709 give up on a second -ESTALE without dropping the range; :5092 and :5206 handle only -EXDEV when zeroing. The range keeps mapping the old extent, which reads zeros once freed.
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

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A range behind blocks it failed to follow is one state, dropped by every way in (07ab65415, 1e0cf9634); a sim scenario kills the mutant task-150.13-behind-unmarked.
<!-- SECTION:FINAL_SUMMARY:END -->
