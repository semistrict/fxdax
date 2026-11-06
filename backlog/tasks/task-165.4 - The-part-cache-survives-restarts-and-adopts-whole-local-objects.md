---
id: TASK-165.4
title: The part cache survives restarts and adopts whole local objects
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: medium
ordinal: 626000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 4. Once local disk holds no full copy, a host's bases live in the part cache; today it is wiped at start and its index is in memory, so an extentd restart would refetch every base. Acknowledged WAL segments must join it whole, without a copy.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Parts kept before a restart are read after it without a GET; the index is rebuilt from the files (their tails), each part checked against its CRC as it is read, a bad one dropped and refetched
- [x] #2 Cache::adopt takes a whole framed object from a local file on the same filesystem (a rename), evictable from then on like any fetched part
- [x] #3 Capacity and the free-space floor hold across restarts and adoptions (tests)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Refactor (own commit): the cache keeps framed objects: an object is kept with its tail, which says where its parts are; a part only with its tail, a tail goes with its object's last part; reads (Cache::tail, Cache::part) check what they give against the tail's CRCs and drop what fails; every change to the files goes through one Files type. fetch.rs calls the new API.
2. Kept across restarts: Cache::open locks its directory (flock), rebuilds its index from its files (each one's tail read and checked, its parts those its file holds data for, by SEEK_DATA/SEEK_HOLE, ordered by mtime), removes files with no valid tail, then evicts to the capacity and the floor.
3. Cache::adopt(key, path): a whole framed object renamed in, its parts evictable as fetched ones; refused (Ok(false), file left) if it doesn't fit.
4. Tests: restart reads without GETs (fetch), torn parts dropped and refetched, capacity and floor across restarts and adoptions; a crash test cutting every change the cache makes to its files at each point (not done, torn: a byte, half, all but one, every other block; and each earlier write or punch lost), reopening and checking every entry reads back right, accounting, no stray file, and the cache working after.
5. OPEN_QUESTIONS decision; macOS scripts/test.sh workspace shuttle; Linux in Docker.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (commits 9bfcdf21b refactor, 272baf4cb kept across restarts, ea319562e adopt; decision in OPEN_QUESTIONS.md):
- The cache keeps objects, not parts. An object's file (fxdax-parts/<key>, sparse, its size) holds its tail, which says where its parts are; a part is kept only with its tail; the tail is read after its parts (touch order) and evicted only with its object's last part, never while one is being written. Cache::tail / Cache::part check what they give (tail CRCs; the part's CRC from the tail, and that the reader's Stored matches) and drop what fails; the fetcher no longer checks the cache's bytes itself. API: open, tail, keep_tail, part, keep_part, forget (object), adopt, bytes. Every change to the files goes through Files (create, resize, write, punch, remove, rename): the crash test's seam.
- Kept across restarts, index rebuilt from the files, no index file: Cache::open flocks the directory (one cache at a time; a second open is WouldBlock, extentd's EIO as for any cache error), walks it, keeps each file whose tail (read from its end) passes Tail::of and whose size is the frame's; parts kept = range all data by SEEK_DATA/SEEK_HOLE; parts shorter than a block (no punch frees them) CRC-checked at open; on a filesystem a probe finds misreporting holes (APFS), every part CRC-checked at open. Other files, links, empty dirs removed. LRU order from mtimes. Then evicts to capacity and floor.
- Cache::adopt(key, path): reads the file's tail and data as open does, renames it in (replacing what is kept at the key, counted once), parts evictable as fetched ones. Refused (Ok(false), file left at its path) past the capacity or floor, or while the key is busy or being removed; a file of no object is InvalidData.

Crash proof: cache::tests::crashes_at_each_change_leave_nothing_read_back_wrong. A workload (5 objects: 2 parts with a short last, one < a block, one a third of a part, one zstd, one adopted; evictions by punch and by removal, a forget, an adoption) is run once uncut to number its 28 changes; then for every change: not made, or (writes and punches) torn as first byte / half / all but last byte / every other block, nothing after it made; plus each write or punch lost with the next change, or all later ones, made (power-cut reordering, nothing is synced). Each case reopens: index invariants (Kept::check), within capacity, no file but its objects', every tail/part read back equals the true bytes or is dropped, bytes == what read back; then a reader keeps all anew and a reopen finds all. ~150 cases, 1.5 s. Mutation: Cache::part returning bytes unchecked turns it (and 3 others) red.

Bugs the tests found, and blind spots:
1. A part shorter than a block, dropped for a bad CRC, can't be punched; after a restart its zeros read as data, so it was indexed again (the crash test, cut at the short part's write). Fixed: such parts are CRC-checked at open. Blind spot: no test reopened after a drop; now the crash test and keeps_what_an_earlier_one_kept do.
2. forget of an object with parts being written left their bytes in the file uncounted, and a later keep_tail revived the object (the concurrency test, extended to reopen after its threads, saw a reopen count more). Fixed structurally: a forgotten object stays doomed until its busy parts settle, then its file goes; nothing of it is kept meanwhile (Object::forgotten).
3. APFS's SEEK_DATA reports the holes a resize left as data once the file has had a hole punched (scratch repro 20/20); a reopen overcounted and evicted real data for room (concurrency test 7/10). Fixed: finds_holes probe at open; on such a filesystem every part is CRC-checked at open. filesystems_find_holes_but_apfs pins it: true on Linux, false on macOS.
Structure: the old cache keyed parts with no object, so a part could outlive its tail and nothing on disk said what a file held; making the tail the object's description is what makes the files self-describing and the rebuild possible.

Overlap with TASK-165.3: none in code (cache.rs, fetch.rs's cache calls and the restart tests only; one line each in extentd/stores.rs and remote.rs's test for Cache::new -> Cache::open). OPEN_QUESTIONS.md gets a decision entry, which 165.3 may also edit.

Runs (on ea319562e): macOS FXDAX_TEST_PARTS="workspace shuttle" scripts/test.sh --no-fail-fast: workspace 2082/2082, every Shuttle part passed. Linux in Docker (OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p fxdax: 636/644; the 8 failures are the environment's: eu-stack missing (stacks_name_where_threads_wait, both *_writes_its_stacks_on_sigquit), grants ENOENT (3), the build stamp, and seccomp's copy_on_write_windows_run_under_firecrackers_seccomp_filters, run against vendor/firecracker as this worktree has it checked out (a2db2b3b0, main's) rather than the d53645971 land-157 names. extent_remote whole on macOS on the refactor commit: 169/169. scripts/upstream-changes.py --check passes. Not done: GCE (DoD #1).

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: fetch's reads_after_a_restart_take_what_the_cache_kept and parts_torn_before_a_restart_are_fetched_again, cache's keeps_what_an_earlier_one_kept and crashes_at_each_change_leave_nothing_read_back_wrong pass in r691 (Linux, where filesystems_find_holes_but_apfs also passes). AC #2: adopts_whole_objects and adoptions_replace_what_is_kept pass in r691. AC #3: capacity_and_floor_hold_across_restarts and adoptions_past_the_capacity_or_the_floor_are_refused pass in r691.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**The part cache's index is rebuilt from its files, which describe themselves (TASK-165.4, 2026-11-05)**

The cache is kept across restarts (doc-14). Decided: no index file. Each object's file holds its tail, which says where its parts are; a part is kept only with its tail, and the tail goes with its object's last part (it is read after them, and spared while a part is being written). `Cache::open` locks the directory (`flock`, one cache at a time), walks it, and keeps each file whose tail, at its end, passes its CRCs; its parts kept are those whose range is all data by `SEEK_DATA`/`SEEK_HOLE` (an evicted part is a punched hole), those shorter than a block (which no punch frees) checked against their CRCs there, and all of them on a filesystem that doesn't find a punched file's holes (APFS reports what a resize left as data once a hole is punched: a probe at open). Files of no object, links and empty directories go. Least recently used order after a restart is the files' modification times. Then it evicts to the capacity and the floor. Every read checks: `Cache::tail` against the tail's CRCs, `Cache::part` against the CRC the tail gives, dropping what fails, so a torn or lost write reads as not kept and is refetched (`cache::tests::crashes_at_each_change_leave_nothing_read_back_wrong` cuts every change at each point). `Cache::adopt` renames a whole framed object in; it returns false and leaves the file where it was if it can't fit.
**Instead:** a durable index (a log of puts and evictions, synced), a second record of what the files already say, which a crash could leave disagreeing with them; or checking every part's CRC at start, reading the whole cache.
**Undoing it:** `Cache::open`'s walk (`Files::walk`, `found`) is the one place; an index file would replace it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The part cache keeps framed objects described by their tails, survives restarts (index rebuilt from the files, parts CRC-checked, bad ones dropped and refetched) and adopts whole local objects by rename. On main: be493fd48, 3e5a3b430, 2300a6b83. Verified: its restart, adoption, capacity and crash tests pass in GCE r691 (workspace 2302 of 2303, the failure TASK-190's); macOS workspace 2118 of 2118 at 752515020; CI 42137709782.
<!-- SECTION:FINAL_SUMMARY:END -->
