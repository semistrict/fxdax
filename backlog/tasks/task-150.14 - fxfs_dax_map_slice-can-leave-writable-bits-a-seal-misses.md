---
id: TASK-150.14
title: fxfs_dax_map_slice can leave writable bits a seal misses
status: Done
assignee: []
created_date: '2026-11-01 16:52'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 529000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed, the window narrow. dax.c:965, with an early return at :959 or :964 skipping fxfs_dax_add_writable. A store to a page sealed meanwhile is refused by the host, which stopped the VM in TASK-99.
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
fxfs_dax_set_writable is the only way a page becomes writable and lists its file first (74f5b2007). Its mutant is unkilled by any seed: TASK-149.26 carries that.
<!-- SECTION:FINAL_SUMMARY:END -->
