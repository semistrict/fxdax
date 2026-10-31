---
id: TASK-115
title: >-
  Guest DAX: evacuee_place's users==1 wait is never woken (fxfs_dax_released
  fires only at 0)
status: Done
assignee: []
created_date: '2026-10-28 12:45'
updated_date: '2026-10-31 06:53'
labels:
  - bug
dependencies: []
ordinal: 432000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28), matches the downgrade-evacuation deadlock of the night of 10-27: fxfs_dax_evacuee_place skips a run when users != 1, but fxfs_dax_put_range wakes (fxfs_dax_released) only when users reaches 0. Any design waiting for users == 1 hangs. Today place skips rather than waits, so it is latent; it matters for TASK-112.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A wait for users dropping to 1 is woken, or the condition is documented as skip-only with an assertion
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: skip-only: fxfs_dax_evacuee_placeable's comment says why it must not wait, and it WARNs if the evacuation's own hold is gone.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Skip-only, as the placing does: documented on fxfs_dax_evacuee_placeable, with a WARN if the evacuation's own hold is gone. No wait for one user exists to be woken.
<!-- SECTION:FINAL_SUMMARY:END -->
