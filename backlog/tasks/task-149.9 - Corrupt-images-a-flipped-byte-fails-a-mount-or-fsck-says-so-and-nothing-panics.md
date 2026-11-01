---
id: TASK-149.9
title: >-
  Corrupt images: a flipped byte fails a mount, or fsck says so, and nothing
  panics
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 510000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest's disk is trusted by nobody after a host failure or a bad upload. Fxfs has cfg(fuzz) Arbitrary derives in 12 files and no fuzz targets. Verify cargo-fuzz's license before adding it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fuzz targets cover journal records, layer files and the super-block
- [ ] #2 Flipped bytes in an image give an error from mount or fsck, never a panic
- [ ] #3 Panics found are fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
