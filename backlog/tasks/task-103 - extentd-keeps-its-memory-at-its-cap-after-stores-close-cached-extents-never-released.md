---
id: TASK-103
title: >-
  extentd keeps its memory at its cap after stores close: cached extents never
  released
status: Done
assignee: []
created_date: '2026-10-25 00:29'
updated_date: '2026-10-25 01:32'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 402000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, ae54e01: one daemon serving VMs one after another, 8 GiB budget: 4,064 MB after the first VM's run, 7,664 after the second, 8,192 from the third on, 7,780 MB held with no store open at a run's end. VMs at the cap ran 123-150 s against 85 s before; 6 of their 7 DAX crashes came at or after the cap. Cached extents of closed stores stay until eviction under pressure.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Related: TASK-104's bound lets go of cache entries (and their pages) past 65,536 extents. Memory up to the daemon's budget is a warm cache by design (forks of a base reuse it); what to free when every store closed, or after idling, is the decision left.

Decision (user, 2026-10-24): keep extents cached up to the daemon's budget, evicting the least recently used under pressure (evict_least_used, TASK-50.3), rather than freeing a store's on close: forks of a base reuse its pages. TASK-104 bounds the entries (65,536), letting go of those nothing maps.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Working as intended: the extent cache keeps pages up to the daemon's memory budget and evicts the least recently used under pressure; entries are bounded by TASK-104. Not freed on store close, by decision, so forks of a base find it warm.
<!-- SECTION:FINAL_SUMMARY:END -->
