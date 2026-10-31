---
id: TASK-119.14
title: 'gcov of fs/fxfs in the guest checks: which dax.c branches no check reaches'
status: Done
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-31 07:43'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 452000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable: know whether evacuee_add failure, map_many fallback, -ENOENT sealed-unwritten, prune renumbering, resize_end on moved pages are ever reached.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A coverage report per GCE run; uncovered DAX branches listed in a task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: FXDAX_GUEST_KERNEL_BUILD=coverage (gcov of fs/fxfs), scripts/guest-coverage.py, results/coverage/coverage.txt per run. r318: dax.c 81.3% of lines, 60.7% of branches. The list: docs/measurements/guest-coverage-2026-10-31/coverage.txt.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A coverage kernel and report per GCE run; the unreached DAX lines listed under docs/measurements/guest-coverage-2026-10-31.
<!-- SECTION:FINAL_SUMMARY:END -->
