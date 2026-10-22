---
id: TASK-92.8
title: Probe builds struct pages for the whole 2 GiB DAX window at every boot
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 283000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
vext_map_window_dax calls devm_memremap_pages over the whole region as MEMORY_DEVICE_FS_DAX, with no altmap and no vmemmap_shift (driver.c:580-587). It runs synchronously in probe (641; no probe_type, 762-770): 524,288 struct pages, a 32 MiB memmap. Boot logs show a 103.6-109.7 ms gap before the probe against 1.9 ms with no window: about a third of the 351 ms kernel boot on every fork boot, plus 32 MiB resident per VM.

Where: guest/linux/drivers/virtio_extent/driver.c:573-593, guest/linux/drivers/virtio_extent/driver.c:641, guest/linux/drivers/virtio_extent/driver.c:762-770.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there; driver.c has not changed since the review. vext_map_window_dax calls devm_memremap_pages once over the whole window as MEMORY_DEVICE_FS_DAX, with no altmap and no vmemmap_shift (guest/linux/drivers/virtio_extent/driver.c:573-595). vext_driver sets no probe_type (driver.c:763-771). Related: TASK-26.14, TASK-26.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
