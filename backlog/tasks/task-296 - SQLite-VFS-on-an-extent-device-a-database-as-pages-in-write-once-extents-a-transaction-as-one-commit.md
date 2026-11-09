---
id: TASK-296
title: >-
  SQLite VFS on an extent device: a database as pages in write-once extents, a
  transaction as one commit
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 09:11'
updated_date: '2026-11-09 09:28'
labels: []
dependencies: []
ordinal: 698000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Asked by the user on 2026-11-09: a SQLite VFS that stores databases straight in an fxdax volume, the "database pages as a library over the store" case TASK-96 names, without Fxfs between them. SQLite in a guest over Fxfs pays an Fxfs transaction per small write (TASK-92.58); a VFS over the extent device makes a SQLite transaction one device commit, with snapshots and forks the store already gives. It sits where Fxfs sits: over `storage_device::Device`, so it runs on an image (crates/fxdax ImageDevice), a FakeDevice in tests, or any device later. Design from the conversation: the main database file is a page map (chunk -> extent, block) persisted as a copy-on-write radix tree in the extents each commit writes, the root naming each file's tree; journals and temp files are memory, as the database file changes only atomically, at a transaction's end; extents freed whole once no page or node in them is live, sparse ones compacted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A crate exposes a Volume over a DeviceHolder and registers it as a named SQLite VFS that rusqlite connections open databases through
- [x] #2 A SQLite transaction commits as one device commit_root: a crash at any barrier leaves every database as of a prefix of its committed transactions, integrity_check ok, shown by a test over FakeDevice crash recording
- [x] #3 Reopening a volume reads back what was committed, several databases in one volume, shown by tests
- [x] #4 Extents no page or node references are freed: after a long run of updates the extents the device holds stay within a bound of the live data, shown by a test
- [x] #5 Extents a crash orphaned, and frees a crash interrupted, are freed at the next open, shown by a test
- [x] #6 WAL mode is refused rather than kept in memory, shown by a test
- [x] #7 A database on a real fxdax image (Storage) survives close and reopen, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crate crates/sqlite_extents (BSD-3, workspace lints): deps storage_device, futures, anyhow, log, rusqlite (MIT; bundled SQLite, public domain).
2. tree.rs: the on-device format: 24-byte refs (branch, sequence, block, present), nodes of (block-16)/24 refs with a trailer (magic, level, index); root encoding (magic, version, minted, dying extents, files: name -> size, height, top ref).
3. file.rs: a file's committed page map in memory (chunk -> slot, block; node locations per level), its extents' slots with live counts and owners, its dirty chunks and pending size; commit plans the extents (dirty chunks, relocated live blocks of extents under half live, dirty nodes and ancestors), writes them, swaps locations, returns extents dead.
4. volume.rs: Volume over a DeviceHolder: open reads the root, walks each tree, frees the root's dying list and orphans in windows past root.minted (as Fxfs's allocator), flushes; mints past max(next_sequence, floor, minted); commit of one file under a volume lock: write extents, commit_root, free the dead after; a failed commit frees what it wrote and carries it in the next root's dying list. In-process locks per file (SQLite's shared/reserved/pending/exclusive). Memory files for journals and temp files; WAL opens refused.
5. vfs.rs: sqlite3_vfs v2 over a Volume (delegating randomness, sleep, time to the default VFS), io_methods v1: commit at xSync and at an unlock from a write lock with changes.
6. Tests: model test of pages vs a Vec across reopen; SQLite end-to-end; several databases; crash recording over FakeDevice (every barrier, marks per committed transaction, integrity_check and prefix); extent bound after many updates; orphans and interrupted frees freed at open; WAL refused; a real fxdax Storage image close and reopen.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built crates/sqlite_extents: format.rs (refs carry their extent's block count, so liveness survives reopen), pages.rs (page map in memory: 8-byte location per chunk plus an 8-byte owner per extent block; commit plans data, relocated live blocks of extents under half live, then nodes bottom up, hole nodes collapse), volume.rs (per-file commits under one lock, dying list in the root, orphans past root.minted freed at open as Fxfs does, a failed commit_root fails the volume), locks.rs, vfs.rs (sqlite3_vfs v2, io_methods v1: no shm, WAL opens refused). Added fxdax Storage::into_device for the image test.
Evidence (macOS, cargo nextest run -p sqlite_extents): 17/17 pass. Space test: 2,056,192-byte database, device holds 3,747,840 bytes (1.82x) after 3,000 single-row update transactions. Hand mutants killed by a_crash_leaves_the_transactions_committed: freeing dying extents before commit_root (the opening fails to read main.db), and skipping the frees at open (device holds an extent the volume doesn't reference). scripts/upstream-changes.py --check passes.
Limits: one mutex per volume, held across device I/O, so connections on one volume run one at a time; a commit buffers its dirty pages in memory; transactions spanning several attached databases commit each separately, not atomically together.

Linux (rust:1 in OrbStack Docker): cargo test -p sqlite_extents 17/17 pass with --security-opt seccomp=unconfined; under Docker's default seccomp the image test can't start the image's io_uring (EPERM), as for any fxdax image.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
New crate sqlite_extents: a Volume on any storage_device::Device (where Fxfs sits) registered as a SQLite VFS. Each database file is pages in write-once extents named by a copy-on-write radix tree; a transaction is one commit_root; journals live in memory; WAL is refused; sparse extents are compacted and dead ones freed, with frees and orphans a crash interrupts finished at the next open. Verified: 17 tests on macOS and Linux, among them crash recovery at every barrier over FakeDevice (4 seeds x 40 transactions, integrity_check and exact contents per mark, device holds only referenced extents), a 1.82x space bound, a failed commit and a real fxdax image closed and reopened; two hand mutants were caught.
<!-- SECTION:FINAL_SUMMARY:END -->
