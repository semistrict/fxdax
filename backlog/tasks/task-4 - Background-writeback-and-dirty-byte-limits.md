---
id: TASK-4
title: Background writeback and dirty-byte limits
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-0
dependencies: []
references:
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/volume.rs
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/volumes_directory.rs
type: feature
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
FxVolume has no parent VolumesDirectory, so the port lacks two upstream mechanisms:

- FxVolume::start_background_task, which flushes all files 70 s after mount and then every 20 s (5 s / 1.5 s under warning / critical memory pressure), and minimizes memory under critical pressure.
- VolumesDirectory::report_pager_dirty, which under critical pressure flushes all volumes before letting more pages be dirtied once dirty bytes reach ~1% of physical memory.

Without them, dirty pages are written back only on explicit sync, connection close or volume termination. The kernel cannot evict dirty pages, so a guest writing without fsync grows host memory without bound, and a host crash loses all data since the last sync.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A mounted volume flushes dirty files periodically without an explicit sync, shown by a test
- [ ] #2 Kernel memory-pressure levels change the background period and trigger minimize_memory, shown by a test
- [ ] #3 Under critical pressure, dirtying past the limit flushes before the page is marked dirty, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
