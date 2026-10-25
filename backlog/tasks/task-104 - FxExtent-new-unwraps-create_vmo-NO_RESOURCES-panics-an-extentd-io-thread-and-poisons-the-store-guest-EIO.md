---
id: TASK-104
title: >-
  FxExtent::new unwraps create_vmo: NO_RESOURCES panics an extentd io thread and
  poisons the store (guest EIO)
status: Done
assignee: []
created_date: '2026-10-25 00:29'
updated_date: '2026-10-25 02:47'
labels:
  - bug
dependencies: []
priority: high
ordinal: 403000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, 2026-10-24, ae54e01: extentd 'thread extentd io panicked at crates/fxfs-platform/src/fuchsia/fxextent.rs:105:79: called Result::unwrap() on an Err value: NO_RESOURCES' (pager.create_vmo), then PoisonError panics at extents.rs 1102, 1493, 1643, 1648 (the store's open and dax locks); the guest's journal terminated (errno 5), PostgreSQL PANICked on fdatasync EIO. Likely the emulated kernel's handle table (1 << INDEX_BITS = 262,144 handles), filled by cached extents' VMOs, each cached extent holding one, small fsync-sized extents under pgbench (their inference, uncounted). Return the error, evict, and count handles.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The cache never let go of an entry, each an FxExtent with a VMO handle: bounded now (MOST_CACHED, 65,536, a quarter of the table): past it, entries nothing else holds (no window maps them), asked for longest ago, go, down to 7/8; dropped past the cache's lock. the_cache_keeps_its_bound (macOS, 225/225 fxdax). create_vmo's unwrap stays (Arc::new_cyclic is infallible): with the bound the table doesn't fill from the cache; making it fallible means FxExtent built without new_cyclic.

FxExtent::new returns its VMO's creation error (made first, receiver set after Arc::new); cached() lets go of half its idle extents and retries once, else ENOMEM; callers propagate, replay skips. caching_fails_when_handles_run_out (macOS; 386/386 fxdax+fxfs-platform), upstream-changes --check passes. a96cfb132. Linux run pending (next GCE workspace run). The arena's BufferSource and the packet port, made lazily on an arena's or the process's first extent, still unwrap NO_RESOURCES: TASK filed.

Linux: GCE r90 workspace green (tree at cfcc877a8). Pushed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FxExtent creation is fallible; the extent cache, out of handles, lets go of half its idle extents and retries, else ENOMEM, instead of panicking and poisoning the store. Cache bounded (MOST_CACHED). Tests: caching_fails_when_handles_run_out, the_cache_keeps_its_bound; macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
