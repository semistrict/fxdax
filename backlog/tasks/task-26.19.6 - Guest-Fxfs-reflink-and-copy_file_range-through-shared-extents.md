---
id: TASK-26.19.6
title: 'Guest Fxfs: reflink and copy_file_range through shared extents'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: medium
type: feature
ordinal: 69000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
btrfs implements remap_file_range (FICLONE, FICLONERANGE, FIDEDUPERANGE). Fxfs already shares extents between objects (fxdax snapshots, small files sharing staging extents), so a clone can be metadata only; cp --reflink and copy_file_range would then copy no data.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FICLONE and FICLONERANGE share extents without copying data
- [ ] #2 Writing either file afterwards leaves the other unchanged
- [ ] #3 copy_file_range uses the same path
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
