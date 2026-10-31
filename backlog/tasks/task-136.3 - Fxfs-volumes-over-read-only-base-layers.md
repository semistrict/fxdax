---
id: TASK-136.3
title: 'Fxfs: volumes over read-only base layers'
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 06:25'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-10 - Fxfs-overlays-for-shared-repository-checkouts.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 470000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A volume whose bottom LSM layers belong to a base is the overlay itself: merging by key and tombstones give its semantics, with no whiteouts and no file-level copy-up. It also keeps mounts cheap, because the sandbox's allocator and root store never learn of the base.

Today `StoreInfo` lists layers as root-store object IDs, a forced major compaction merges every immutable layer, and each layer counts towards metadata reservations at 1.5x its size. See doc-10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A volume made from a base volume's `StoreInfo` and layer extents reads as the base did, without replaying the base's journal, in time independent of the base's size
- [ ] #2 Writes, renames and deletes in the volume are private to it, and reads see them over the base
- [ ] #3 No compaction, forced major compactions included, rewrites or releases a base layer, and tombstones over base records stay
- [ ] #4 Base layers count towards no metadata reservation, and base extents are never freed, reflinks of them included
- [ ] #5 fsck passes on a filesystem holding such a volume, and it survives crash and journal replay
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
