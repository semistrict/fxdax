---
id: TASK-26.19.11
title: 'Guest Fxfs: large folios and O_DIRECT for files not on DAX'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: enhancement
ordinal: 74000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs enable large folios in their page caches; Fxfs uses order-0 folios and has no direct I/O path (ext4 also offers MAP_SYNC on DAX via FOP_MMAP_SYNC).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Files not on DAX use large folios
- [ ] #2 O_DIRECT reads and writes work
- [ ] #3 MAP_SYNC mmaps of DAX files are accepted and honoured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
