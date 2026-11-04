# Writable DAX: the window page a store reaches is the open extent's block

**Tried:** 2026-10-14 to 2026-11-04. From TASK-26.14's file data through the DAX window
(c552aee13, 2026-10-14) to the deletion in batch 2026-11-04e (branch batch/2026-11-04e, TASK-164.7
and 164.8): 169 commits to guest/linux/fs/fxfs/dax.c alone. Its bugs and fixes fill TASK-79, 92,
99, 101, 112, 119, 121, 133, 140, 147, 150 and 153 (153.15 the seal asks, 150.20 the open-page
budget), and its specs and tests TASK-44, 119 and 149.14 (guest/sim). Replaced by copy-on-write DAX
(backlog doc-13, TASK-164), chosen in docs/reviews/2026-11-04-first-principles.md.

**Idea:** doc-2's provisional write path: DAX writes with barriers on commit, no copy and no request
per write. A writable `MAP` (`DAX_WRITE`, `MAP_WRITE`) put an open extent's pages in the window, so
a store through a mapping, or a `write(2)`'s copy, was the write: the page *was* the extent's
block, and the next barrier (`COMMIT`) sealed it in place. Write-once extents (doc-4) stayed
write-once: the host made the window's mappings of an extent read-only as a commit sealed it.

**Result:** every fsync seals every open extent, so the next store to any page found it sealed and
had to move its block to a new open extent, in the fault path, racing the next seal. What that took:
a gate keeping seals and stores apart (`fxfs_dax_store_begin`), seals' write-protects of every
writable PTE, moves, staging extents and their regions, restaging and evacuation after each seal,
`write(2)` through the staging region with lagging pages, held copies, 57 sites using errors to
break the fault/semaphore/gate/seal cycle (`-EXDEV` retries, `FXFS_DAX_MAX_DROPS`), Fxfs's
`txn_map_write*`, `ExtentAccess`, `DaxWrite`, `Allocator::restage`; on the host, open extents'
memory pinned until a seal, its budget refusing a `MAP` with `ENOMEM` (TASK-150.20), and asks that
an idle guest seal (`F_SEAL`, `seal_asked`, `SEALED`, TASK-153.15). About 25k lines, 7 kernel
patches (0005, 0008 and 0009 for writes that move pages alone), 3 of 4 TLA+ specs (`specs/dax`,
`dax-region`, `extent-open`).

- **Speed:** DAX at 0.51-0.61x no DAX under pgbench, 1,012 tps against 1,736 (doc-13; doc-9's
  analysis: evacuation holding each file's semaphore ~650 us after nearly every commit, 32 s of a
  60 s run; 10k stores a run waiting ~480 us each for seals; 58 us of extentd's 126 us commit
  write-protecting windows).
- **Bugs:** ~47% of bug-titled tasks were writable DAX's, and 37-39 of the 65 dead ends before this
  one (`map-waits-for-a-seal`, `seal-places-staging-pages`, `pages-moving-to-staging-regions`,
  `no-evacuation`, `durable-open-staging-extents`, `one-retry-driver-for-writable-dax`,
  `open-extents-take-pages-ahead`, the `theory-*` of moves, seals and open maps, ...). Data lost
  under seals (TASK-121, 133), hangs of a fault against a seal (TASK-79.12, bdc653701), soft lockups
  (TASK-140), refused stores stopping the VM (TASK-99, 101), and guest/sim's first findings
  (TASK-149.14) were all a mapped page changing under a seal.

DAX's benefit was reads all along: 22 MB of guest page cache per VM against 469 MB (TASK-112), a
first read of /usr in 0.69 s against 2.65 s. Copy-on-write DAX keeps those and writes as no DAX does.

**Why it's dead:** extents are write-once, sealed by the next barrier, and every fsync is a barrier:
so a seal changes what a mapped page is. A page the guest maps writable is an open extent's block
only until the next fsync anywhere in the filesystem; after it, a store needs a new block, which
moves the page under every PTE and every reader that maps it, while the next seal comes. Each
mechanism above managed one consequence of that, and each brought its own races. No locking scheme
removes it: the cost is structural, in the fault path, on every first store after every fsync.

**Reopen if:** a store can land without moving a mapped page across a seal: extents writable in
place past a commit (a store format whose extents grow across commits, doc-9's "durable-but-open"
staging extent, with prefix-level protection of window pages and seal-on-share in forks, snapshots
and followers); or fsyncs that seal only what they must, not every open extent. And even then only
if copy-on-write DAX's cost, a host fault on each page's first store (~54-110 us nested) and
writeback's copy, shows in a measurement as worse than no DAX's writes.
