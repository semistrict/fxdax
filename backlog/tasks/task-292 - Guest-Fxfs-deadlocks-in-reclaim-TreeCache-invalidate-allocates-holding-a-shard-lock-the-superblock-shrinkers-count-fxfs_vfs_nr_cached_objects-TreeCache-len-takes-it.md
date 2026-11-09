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
updated_date: '2026-11-09 13:07'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix c42c8e2a6-line (pushed with TASK-257): Shards counts its items in an AtomicUsize updated under the shard's lock (added/removed, enforce), len() reads it lock-free, shrink() try_locks and passes over a held shard. Host test test_counted_and_shrunk_under_a_shards_lock hangs on the old len (verified: SLOW >60 s, killed) and passes on the fix. GCE debug fxfs-guest-checks-dax-delay with the fix: t257dbg3 2 rounds (round 2 lost to TASK-299), t257ws4 8 rounds (round 8 lost to TASK-299): 10 rounds, no hang; AC #3 (20 rounds) open, TASK-299 stopping runs.

2026-11-09, ubu1 (non-root): debug kernel built (FXDAX_GUEST_KERNEL_DEBUG=1), fxfs-guest-checks-dax-delay alone with FXDAX_E2E_ROUNDS=20 (prebuilt, as test-e2e.sh's own pre-tests need root, TASK-307): round 1 failed in 1 s, the extent device's userfaultfd refused (/dev/userfaultfd root 0600, Firecracker's fallback syscall killed by seccomp, TASK-308). Blocked on ubu1's setup: /dev/userfaultfd must be readable and writable by ramon (udev rule for the kvm group). Worktree ~/src/fxdax-t292 and kernel ~/fxdax-guest-kernel-debug kept on ubu1 for the re-run.
<!-- SECTION:NOTES:END -->
