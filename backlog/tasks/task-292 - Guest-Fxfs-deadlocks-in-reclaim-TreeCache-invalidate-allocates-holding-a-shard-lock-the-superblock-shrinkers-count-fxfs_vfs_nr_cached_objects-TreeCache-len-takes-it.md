---
id: TASK-292
title: >-
  Guest Fxfs deadlocks in reclaim: TreeCache::invalidate allocates holding a
  shard lock, the superblock shrinker's count (fxfs_vfs_nr_cached_objects,
  TreeCache::len) takes it
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 08:41'
updated_date: '2026-11-09 09:13'
labels:
  - bug
  - fxfs
dependencies: []
priority: high
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE t257dbg2 (debug kernel, main 142195ae2 + TASK-257, fxdax-keep-n2-standard-4-us-west1-a-1), fxfs-guest-checks-dax-delay round 1 hung (TIMEOUT 711 s) after timestamps_clamped: hung tasks from 133 s. A transaction's apply_mutation -> TreeCache::invalidate holds a shard's mutex while LinkedHashMap::insert allocates (fxfs_rt_alloc: kvmalloc GFP_KERNEL|__GFP_NOFAIL) -> direct reclaim -> shrink_slab -> super_cache_count -> fxfs_vfs_nr_cached_objects -> fxfs_cached_objects -> TreeCache::len, which locks every shard: the same mutex, a self-deadlock; kswapd and other tasks then block on it too (fxfs_rt_futex_wait in Mutex::lock_contended). super_cache_count counts whatever the allocation's GFP flags (only the scan checks __GFP_FS), so fxfs_nofs does not cover it; and a scan from an allocation not in a NOFS scope would lock a shard in TreeCache::shrink the same way. Fix: count the cache's items in an atomic (len without the shard locks) and have shrink skip a shard it can't try_lock. Results: run-t257dbg2-20261109T075130Z-1efe7cc2, e2e/fxfs-guest-checks-dax-delay/round-1 console.txt.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 TreeCache::len takes no lock, and TreeCache::shrink no lock it would wait for
- [ ] #2 A host test: an allocation under a shard's lock that calls the cache's len and shrink returns (no deadlock)
- [ ] #3 fxfs-guest-checks-dax-delay passes 20 rounds on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
