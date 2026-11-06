---
id: TASK-92.45
title: Measure segment cleaning under real guest churn on GCE (TASK-79.2)
status: Done
assignee: []
created_date: '2026-10-23 03:10'
updated_date: '2026-11-06 06:50'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: task
ordinal: 329000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.2's cleaner (round 1) copies the live remainder out of mostly-dead segments behind the commits, and extentd runs a pass when Store::cleaning_due. It was measured only on macOS (APFS, F_NOCACHE), not on a network persistent disk where framing and fsync cost more, and never under a real guest: how often cleaning is due under pgbench and fs-bench, what a pass costs the guest's requests, and what a write burst past a segment's bound costs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench and fs-bench on GCE report how often cleaning runs, its copies, and the guest's request latency while it runs
- [ ] #2 The numbers are compared with main before TASK-79.2
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Moot after TASK-165.8 (a31f82a98): the cleaner this was to measure (TASK-79.2's, Store::cleaning_due and extentd's cleaning behind commits) is deleted; nothing cleans a store locally. What the collector costs is the bucket's, measured by neither of this task's criteria.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as moot: TASK-165.8 deleted local segment cleaning, the thing to be measured.
<!-- SECTION:FINAL_SUMMARY:END -->
