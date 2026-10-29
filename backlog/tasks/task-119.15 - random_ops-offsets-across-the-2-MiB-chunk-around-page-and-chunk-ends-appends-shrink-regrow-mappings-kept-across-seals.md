---
id: TASK-119.15
title: >-
  random_ops: offsets across the 2 MiB chunk, around page and chunk ends,
  appends, shrink/regrow, mappings kept across seals
status: Done
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-29 20:11'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 453000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra: MAX is 1 MiB, below the DAX chunk; mappings are made and dropped per op.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Ops reach past 2 MiB and keep a mapping across seals; histories saved for replay
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On main: random_ops's file reaches 4 MiB, across two 2 MiB chunks; its offsets are drawn near a chunk's end, a page's end and the file's end as often as anywhere; appends among its operations; one mapping of the file kept across every operation (seals and truncations among them), its stores and the whole-file checks going through it; its operations saved (random-ops.history, its seed first), which the host finds. GCE r251: fxfs-guest-checks, -dax, -dax-delay, -dax-small 2 rounds each passed, the host comparing the file with its model and finding the history.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
random_ops reaches past 2 MiB with offsets drawn near chunk, page and file ends, appends, a mapping kept across seals and truncations, and a saved history. Verified by GCE r251 (all four checks suites, 2 rounds).
<!-- SECTION:FINAL_SUMMARY:END -->
