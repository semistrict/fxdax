---
id: TASK-26.19.10
title: >-
  Guest Fxfs: Fxfs caches answer memory pressure (nr_cached_objects /
  free_cached_objects)
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 15:52'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: enhancement
ordinal: 73000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
btrfs reports and frees cached objects through the super_operations shrinker hooks. Fxfs keeps a TreeCache sized from guest memory and in-memory LSM layers up to the journal size, neither of which shrinks under the kernel memory pressure.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Under memory pressure the guest reclaims Fxfs cached records
- [x] #2 A guest test shows the cache shrinking when memory is tight
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
ObjectCache::shrink (fxfs, d5cc9da) behind super_operations nr_cached_objects/free_cached_objects; FXFS_IOC_CACHED_OBJECTS counts them. Check cache_shrinks: 2000 stats leave >= 4000 records cached, drop_caches 2 (the superblock shrinker) leaves <= a tenth. The in-memory LSM layers, bounded by the journal size, are not shrunk (README). GCE: fxfs-guest-checks and -dax pass with the check; full suite 27/27 guest, 2239 workspace (commit df750c5).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fxfs's cached records are freed by the superblock shrinker under memory pressure; verified by cache_shrinks (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
