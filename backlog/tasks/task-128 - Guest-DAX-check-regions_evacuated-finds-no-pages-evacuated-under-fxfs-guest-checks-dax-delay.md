---
id: TASK-128
title: >-
  Guest DAX check regions_evacuated finds no pages evacuated under
  fxfs-guest-checks-dax-delay
status: To Do
assignee: []
created_date: '2026-10-29 02:00'
updated_date: '2026-10-29 03:05'
labels:
  - bug
  - testing
dependencies: []
priority: medium
ordinal: 463000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
r208 at d716667e7, fxfs-guest-checks-dax-delay round 1: regions_evacuated failed 'no pages evacuated': window_maps_evacuate did not grow across its fsyncs (11179 overall). The check writes with write(2) only. It passed in earlier delay runs (cae97000a, 14af2c666): timing-dependent, as the injected delays may hold evacuation (grow_wq) past the check's end. Results: /tmp/fxdax-r208-final.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is known: evacuation not run within the check, or not finding the regions' pages
- [ ] #2 regions_evacuated passes 10 rounds of fxfs-guest-checks-dax-delay
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Bisected: not kernel patch 0008 nor the eager unmap (full delay suite fails without both); the check alone passes at HEAD; it passed in the full suite at cae97000a and 14af2c666. Since 7f0b65266 seal_race read /proc/self/pagemap for every byte, slowing it: its sealer sealed more, and the window's regions were spent when regions_evacuated ran (every round failed). 186bbf0a8 reads it only on a mismatch: 5 of 6 rounds pass. Left: round 3 on r208 still found none evacuated, the window's region space free 176 MB but its largest free run ~2 MB: fragmented by the checks before, the writes' pages finding no region (window_regions_full high). Next: whether region allocation needs more than the largest free run, and whether the check should make room first or run before the seal-heavy checks.
<!-- SECTION:NOTES:END -->
