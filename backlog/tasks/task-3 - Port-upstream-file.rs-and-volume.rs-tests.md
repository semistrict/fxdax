---
id: TASK-3
title: Port upstream file.rs and volume.rs tests
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
updated_date: '2026-09-29 00:49'
labels: []
milestone: m-0
dependencies: []
references:
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/file.rs
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/volume.rs
type: task
ordinal: 3000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Step 5 runs pager.rs and paged_object_handle.rs tests verbatim, but FxFile's own tests (file.rs) and FxVolume's (volume.rs) are not ported. They cover truncation, allocation, attributes, flush and volume lifecycle, which the virtio-fs server relies on; without them regressions in the port go unnoticed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 file.rs tests run through kernel-cfg.py and pass; each disabled test lists its reason
- [ ] #2 volume.rs tests run through kernel-cfg.py and pass; each disabled test lists its reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-4 ported the three background-flush tests of volume.rs and three tests of volumes_directory.rs (test_volume_creation, test_dirty_pages_accumulate_in_parent, test_flush_before_mark_dirty_under_critical_memory_pressure).
<!-- SECTION:NOTES:END -->
