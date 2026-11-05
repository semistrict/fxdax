---
id: TASK-164.8
title: 'Docs: doc-2, doc-4, doc-9, doc-12, README, and writable DAX''s dead end'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-05 04:54'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 618000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 4's docs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each says what copy-on-write DAX does, nothing of seals moving pages
- [x] #2 dead-ends/ holds writable DAX, with why
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
doc-2, doc-4, doc-9, doc-12 and README revised for copy-on-write DAX (rg finds no MAP_WRITE, evacuation, restaging or writable mappings described as current); dead-ends/writable-dax.md and dead-ends/one-retry-driver-for-writable-dax.md hold writable DAX and why.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Docs say what copy-on-write DAX does: doc-2, doc-4, doc-9 (superseded but for sections 1 and 3), doc-12, README; dead-ends/writable-dax.md records writable DAX and why it went. Landed with batch 5 (09b36991a).
<!-- SECTION:FINAL_SUMMARY:END -->
