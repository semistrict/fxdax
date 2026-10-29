---
id: TASK-119.7
title: >-
  TLA+ spec of DAX evacuation: pages, regions, moved, seals, gather/MAP/place,
  writers, truncate, unordered host replies
status: Done
assignee: []
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 20:22'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 443000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
specs/dax models one page and specs/dax-region one region; evacuation, prune and unordered host replies are unmodelled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 specs/dax-evacuate model-checks in make check-tla
- [x] #2 A mutation per rule breaks an invariant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
340888174: specs/dax-evacuate/DaxEvacuate.tla models one page of a file: writes, holding the file's semaphore, have Fxfs move its block to the staging extent open and map its region's slot, then take the page as there and store; seals seal the staging extent open (no store in the gate); evacuations gather a page in a sealed region, map its block in the range's own space and place it there; prunes let sealed regions' mappings go; truncation empties the page; the host's MAP replies come in any order. Invariants: TypeOK, ViewConsistent (the guest reads where it shows the page what was written last) and Durable (Fxfs's block holds it). DaxEvacuate (2 staging extents, 2 writes, 253 states) and DaxEvacuateDeep (3, 3, 1773 states) hold; each rule's mutation violates ViewConsistent: NoEvacuationLocked (TASK-101's evacuation_unlocked), NoPlaceAfterReply, NoPruneKeepsMoved (TASK-128's prune_in_use). check-tla.sh runs them with every model (make check-tla); its tail -1 now sed, as tail hangs in this environment. A full make check-tla run here outlasted a 280 s cap on an existing model; the five dax-evacuate models pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/dax-evacuate model-checks the guest's evacuation protocol (writes moving a page's block, seals, gather/MAP/place, prunes, truncation, unordered MAP replies) in make check-tla; each of its rules' removal breaks ViewConsistent.
<!-- SECTION:FINAL_SUMMARY:END -->
