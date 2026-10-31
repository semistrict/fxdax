---
id: TASK-140
title: >-
  Guest DAX: soft lockup in fxfs_dax_map_write under xfstests generic/111,
  kernel WARNs at dax.c:2074 and fs/dax.c:461
status: To Do
assignee: []
created_date: '2026-10-31 16:32'
labels:
  - bug
  - dax
dependencies: []
priority: high
ordinal: 486000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r342, xfstests-fxfs-guest-dax: generic/111 (reflinks): xfs_io stuck 26 s in fxfs_dax_map_write, which loops: soft lockup, panic. Before it, generic/013 (fsstress): WARNING fs/fxfs/dax.c:2074 fxfs_dax_assert_mapped_within: mapped pages past a region's, from fxfs_dax_map_chunk's fxfs_dax_move, in a write; generic/075 (fsx): WARNING fs/dax.c:461 dax_associate_entry from a read fault: a page in two files' entries, reflinks under DAX, which dax_associate_entry asks to be shared. Logs: /tmp/fxdax-r342-xfstests/e2e/xfstests-fxfs-guest-dax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 generic/013, 075 and 111 pass under DAX, no WARN, no lockup
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
