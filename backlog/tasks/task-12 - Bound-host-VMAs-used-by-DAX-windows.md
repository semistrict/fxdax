---
id: TASK-12
title: Bound host VMAs used by DAX windows
status: To Do
assignee: []
created_date: '2026-09-28 19:56'
updated_date: '2026-10-19 20:10'
labels: []
milestone: m-1
dependencies: []
priority: high
ordinal: 12000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host MMU maps each kernel page with its own mmap, and the Linux uffd second stage duplicates each mapped page again, so a fully populated window (512 MiB = 131072 pages of 4 KiB) needs more VMAs than vm.max_map_count (65530) allows, and mmap then fails with a panic. Coalesce contiguous physical runs or map windows differently so large windows stay within the limit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test populates a window larger than vm.max_map_count pages on Linux without failing
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
