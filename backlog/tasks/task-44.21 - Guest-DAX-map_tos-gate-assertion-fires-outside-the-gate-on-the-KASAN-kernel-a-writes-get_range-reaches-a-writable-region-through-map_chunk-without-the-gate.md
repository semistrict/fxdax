---
id: TASK-44.21
title: >-
  Guest DAX: map_to's gate assertion fires outside the gate on the KASAN kernel:
  a write's get_range reaches a writable region through map_chunk without the
  gate
status: To Do
assignee: []
created_date: '2026-10-29 20:54'
labels:
  - dax
  - bug
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 469000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r257 (main 88032eb67, FXDAX_GUEST_KERNEL_BUILD=kasan: KASAN, KFENCE, lockdep), fxfs-guest-checks-dax after random_ops: WARNING at fs/fxfs/device.c:220 fxfs_device_assert_in_gate, from fxfs_dax_map_to <- fxfs_dax_reach <- fxfs_dax_move <- fxfs_dax_map_extents <- fxfs_dax_map_chunk <- fxfs_dax_get_range <- fxfs_dax_iomap_write. map_to asserts the gate when the region is writable, at its entry, before it finds the region mapped far enough already (no MAP sent). map_chunk enters the gate only if an extent Fxfs gives is open: so either the region was mapped as far as asked and nothing was sent (the assertion too broad), or the region was writable while its extent's piece came back sealed, and a writable MAP went outside the gate, which a seal may cross (a real race). The debug kernel's runs (r80, r81) never hit it; KASAN's slowness widened the window. Artifacts: /tmp/fxdax-r257/e2e/fxfs-guest-checks-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The assertion fires only where a writable MAP is sent outside the gate
- [ ] #2 Whether a writable MAP is sent outside the gate is known: fixed if so
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
