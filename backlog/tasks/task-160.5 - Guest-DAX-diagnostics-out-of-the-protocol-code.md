---
id: TASK-160.5
title: 'Guest DAX: diagnostics out of the protocol code'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 03:07'
labels:
  - structure
dependencies:
  - TASK-164
parent_task_id: TASK-160
priority: low
type: task
ordinal: 576000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 900 of dax.c's lines are diagnostics (shadow, trace, move checks, seal verification, stray accesses) woven into the protocol: iomap_write keeps before/before_at/before_where beside held. The diagnostics have had bugs of their own: TASK-119.21, fb440d189, 19031ef69, 119.3. Proposal: dax_diag.c behind a few hooks gated by static keys; held stays (it is protocol, TASK-79.11).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 iomap_write holds no diagnostic state
- [ ] #2 checks-dax passes with the diagnostics on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: waits for TASK-164 (read-only DAX), which changes what this covers; re-scope it then.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Obsolete: made so by the deletion of writable DAX (37c331a6c), which took out the diagnostics this was about: the shadow (dax_shadow), the move checks (dax_check_places), seal verification, and iomap_write's before/before_at/before_where state beside held, with held itself (dax.c 3,264 lines removed). What is left is test hooks apart from the protocol's state: the trace ring (fxfs.dax_trace, guest/linux/fs/fxfs/dax.c:518-600, four fxfs_dax_trace calls), fxfs.dax_stray (dax.c:1911-1948) and pause points (dax.c:158-, 17 fxfs_dax_pause calls). fxfs_dax_iomap_write (dax.c:2496-2560) keeps no diagnostic state, but AC1 is left unchecked as no check or test was run for it; AC2 was not run.
<!-- SECTION:FINAL_SUMMARY:END -->
