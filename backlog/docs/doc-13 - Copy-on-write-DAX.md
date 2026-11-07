---
id: doc-13
title: Copy-on-write DAX
type: specification
created_date: '2026-11-04 07:55'
updated_date: '2026-11-07 03:00'
---
# Copy-on-write DAX: the window maps sealed extents only; a store gets a private copy

**Landed, 2026-11-04 (batch 2026-11-04e): the only DAX.** Writable DAX, the window page an open
extent's block, was deleted outright at stage 4, with no flag and no A/B batch (the user: "we can
always revert it"): dead-ends/writable-dax.md. `VIRTIO_EXTENT_F_DAX` means copy on write; the
`DAX_WRITE` and `DAX_COW` feature bits, `MAP_WRITE`, `F_SEAL`, `seal_asked`, `SEALED` and the
`fxfs.dax_cow` parameter are gone; `MAP_ZEROS`, `PRIVATE` and `private_max` stay. What follows is
the design as it was written, its stages and lists marked as they went.

Design, 2026-11-04 (TASK-164). The user chose read-only DAX that day (docs/reviews/2026-11-04-first-principles.md,
section 1, option a), then asked for one case it must meet: a process maps a 400 GB file
`MAP_SHARED` read-write and writes ~5% of it in this fork; the other 95%, blocks earlier forks
wrote included, stays DAX, and only the written pages cost memory. A file leaving DAX whole at its
first write can't meet that, so this is the review's option b in its simplest form: the window maps
sealed extents only, read-only to the store; a store to a page gives the VM a private copy of it in
host memory, at the same window address; writeback copies dirty pages into new extents, which Fxfs
allocates then (delayed allocation, as ext4 and XFS do), and commits after their bytes. Files
created in a mount go through the guest's page cache, as without DAX, until their inode is next
read in. Kernel references are to the guest's kernel, Amazon Linux's 6.18
(`kernel6.18-6.18.54-121.166.amzn2023`, scripts/build-guest-kernel.sh).

## Why

A window page today *is* the open extent's block, and every fsync seals every open extent, so the
next store to any page moves its block, in the fault path, racing the next seal. That costs DAX
0.51-0.61x no-DAX under pgbench (1,012 against 1,736 tps), about 25k lines, 7 kernel patches, 3 of
4 TLA+ specs, ~47% of bug-titled tasks and 37-39 of 65 dead-ends. DAX's benefit is reads: 22 MB of
guest page cache per VM against 469 MB (TASK-112), a first read of /usr in 0.69 s against 2.65 s.

What made writes hard is that a seal changes what a mapped page is. Here nothing a guest stores to
is ever an extent, and nothing mapped is ever open: a seal (`COMMIT`) changes nothing in the window.

## Invariants

1. **The window maps sealed extents, or zeros.** An open extent is never mapped (`MAP` of one
   fails `EPERM`). Extents are written only by `WRITE`.
2. **A window page is one of three:** a sealed extent's page, shared by every window that maps
   it; zeros; or a private page, the VM's own, which no extent names. A store to either of the
   first two makes it private, holding what it held. Only a `MAP` or `UNMAP` over it, which the
   guest sends, ends a private page. A private page's window address never changes: no store moves
   a page, so no PTE of the guest's changes for a store.
3. **The guest's file semaphore, held exclusively, sees every page of the file outside its dirty
   and writeback sets with no writable PTE and no copy into it under way.** Only then may a page
   go back to an extent (rebinding) or leave the window.
4. **At most `private_max` private pages a device.** The guest keeps under it; the host refuses a
   store past it, and that VM alone stops (159, `AccessRefused`, as a refused store does today).

1 and 2 are the host's to enforce; 3 is the guest's one locking rule, replacing the gate; 4 bounds
memory without asking other VMs anything.

## Linux facts that decide it

- **`S_DAX` is set as an inode is read in, never on a live inode.** ext4:
  `ext4_set_inode_flags(inode, init)` sets `S_DAX` only `if (init && ext4_should_enable_dax(inode))`
  (fs/ext4/inode.c:5037-5059); a change of `FS_XFLAG_DAX` only marks the inode
  (`ext4_dax_dontcache` -> `d_mark_dontcache`, fs/ext4/ioctl.c:551-563), and a change of the
  journalling mode of a DAX inode fails `EBUSY` (fs/ext4/ioctl.c:691-695). XFS: "S_DAX can only be set
  during inode initialization" (fs/xfs/xfs_iops.c:1399), the flag change `d_mark_dontcache`
  (xfs_ioctl.c:580-597). virtio-fs, per inode from its server's `FUSE_ATTR_DAX`, likewise
  (`fuse_dax_dontcache`, fs/fuse/dax.c:1327). `d_mark_dontcache` sets `I_DONTCACHE`
  (fs/dcache.c:787), so the last `iput` evicts the inode (fs/inode.c:1889) and the next read-in
  picks the new mode: "won't take effect until the files are closed by all processes"
  (Documentation/filesystems/dax.rst). ext4's one live switch of an inode's address-space
  operations, the journalling flag, flushes, locks out writers and swaps aops
  (`ext4_change_inode_journal_flag`, fs/ext4/inode.c:6591) and is refused for DAX inodes.
- **One `address_space` can't hold both DAX entries and folios.** `dax_mapping()` is
  `IS_DAX(mapping->host)` (include/linux/dax.h:285), per inode; `grab_mapping_entry` fails a fault
  `-EIO` on a non-value entry (fs/dax.c:668); `dax_writeback_one` WARNs "A page got tagged dirty in
  DAX mapping?" (fs/dax.c:1114); `dax_layout_busy_page_range` WARNs on one (fs/dax.c:791);
  truncation WARNs on DAX value entries left (mm/truncate.c:79-95); `__filemap_add_folio` takes a
  value entry for a workingset shadow (mm/filemap.c:924, 1000); `vma_is_dax()` is per file
  (include/linux/fs.h:3820). Mixing would take patches through fs/dax.c, mm/filemap.c and
  mm/truncate.c, two lock domains per index (DAX entry locks, folio locks) and transitions both
  ways under `invalidate_lock`: a new kernel feature, not a fix.
- **A write fault keeps its page's pfn if the filesystem's iomap does.** A store to a page a
  read fault mapped goes through `->page_mkwrite`/`->pfn_mkwrite` to `dax_iomap_fault` with
  `IOMAP_WRITE`; the iomap's address is the pfn inserted, writable, the entry tagged dirty, the
  inode dirtied. Not `IOMAP_F_SHARED`, nothing is copied around or replaced. A store to a PMD a read
  mapped splits it (`wp_huge_pmd`, mm/memory.c:6098-6107) when `->huge_fault` falls back, and the
  PTE fault uses the PMD's entry as it is (`grab_mapping_entry` downgrades only zero and empty
  PMD entries, fs/dax.c:672-678).
- **`write(2)` to DAX needs `IOMAP_MAPPED`, and `IOMAP_F_NEW` invalidates** the range's entries
  (fs/dax.c:1611-1631), the zero-page entries of holes among them. With holes mapped to the
  window's zeros, no write says `IOMAP_F_NEW`.
- **Writeback write-protects, under the entry's lock, and tells the filesystem nothing.**
  `dax_writeback_one` locks the entry, `pfn_mkclean_range`s every mapping, `dax_flush`es (a no-op
  without a write cache on the dax device), clears the dirty tag (fs/dax.c:1103-1190). So Fxfs keeps
  its own set of what to write.
- **`MAP_PRIVATE` stores copy into anonymous guest memory** with no help from the filesystem:
  `do_cow_fault` allocates the page (mm/memory.c:5745-5767) and `dax_fault_cow_page` copies the DAX
  page into it (fs/dax.c:1769).
- **Long-term pins of fsdax pages are refused** (`FOLL_LONGTERM`, mm/gup.c:1213), as today.

## Options

1. **Host copy-on-write, per page (chosen).** Above. Every kernel path it uses is stock fs/dax.c's.
2. **Guest page cache per page, in the DAX inode's mapping:** unwritten pages DAX entries, written
   ones folios. Refused by the kernel facts above.
3. **A carved pool of guest RAM as a second fsdax device for written pages,** fs/dax.c's
   `IOMAP_F_SHARED` copy-on-write moving a page from the window to the pool on its first store:
   a move of the page, its pfn and PTEs, which is the machinery being removed; and guest RAM set
   aside at boot whether written or not.
4. **Per file: a written file leaves DAX.** Linux allows the switch only at eviction, so a write-open
   of a DAX inode in use either waits for every other user to go, fails, or switches the live inode
   (unmap every mapping, wait out pinned pages, drop every DAX entry, swap aops, under `i_rwsem` and
   `invalidate_lock` held exclusively), which no in-tree filesystem does. Compared below.
5. **Per file, decided at creation and read-in (adopted, beside 1):** an inode created in this
   mount is a page-cache inode until it is next read in; any other is DAX. No switch: `S_DAX` is set at read-in only, as
   the kernel expects. Created files take the no-DAX path, the fastest for writing streams of new
   pages (no host fault per page, below); the files that hold shared data were read in.

## The device (doc-4 changes)

- **`VIRTIO_EXTENT_F_DAX` copies on write** (bit 0; `DAX_WRITE`, bit 1, went; stages 1-3 offered
  this as `DAX_COW`, bit 10, beside it, gone too at stage 4): a store to a page of the window gives
  the device a private page holding what the page held, mapped writable at the same address, until
  a `MAP` or `UNMAP` replaces it. Not with `READ_ONLY`: a follower's stores stay refused.
- **`private_max`** (configuration, with `DAX`): the most private pages the device holds at
  once. A store past it is refused, and the VM stops (159). The host sets it as it opens the device,
  and counts it against the tenant's memory then, not as pages come: a store never waits for memory.
- **`MAP_ZEROS`** (a `MAP` flag): maps zeros, `id` ignored: holes and past-the-end pages of a range,
  so that a store to one is a private page of zeros (no copy).
- **`PRIVATE`** (request): makes `len` bytes at `window_offset` private now, as stores would, and
  maps them for the guest ahead of its accesses, as `MAP_AHEAD` does; `ENOMEM` past `private_max`.
  For a `write(2)` of many pages not yet private: one round trip, rather than a host fault each.
- **`MAP`** of an open extent fails `EPERM`, read-only or not (`MAP_WRITE` goes). Every `MAP` is
  read-only; `ENOMEM` goes from it.
- **Sealing** (`F_SEAL`, `seal_asked`, `SEALED`) goes: nothing a guest holds pins what another
  needs.
- **`COMMIT`** seals as before; it changes nothing in any window.
- `WRITE`, `READ`, `FREE`, `MAP_MANY`, `MAP_AHEAD`, `UNMAP`, followers, snapshots unchanged, but
  for: a snapshot is refused while the device has a private page (the guest's freeze rebinds them
  all, below), so a snapshot's window still maps only extents and zeros. *As built:* the store
  refuses a snapshot's point (`EBUSY`, saying how many), and the window's mappings, while its window
  holds a private page (`ExtentStore::point`, `window_mappings`, TASK-203): fxdax's Firecracker
  takes the point first, so its snapshot fails before anything is made, and the VM goes on as it
  was. The guest's freeze warns of the private pages it couldn't rebind (`no snapshot`): those a
  writeback that found no memory kept dirty (TASK-207), or a rebind refused.

## The host

- **A store to a read-only window page,** a fault by a vCPU's thread (userfaultfd's thread ID, as
  the window tells a vCPU's refused store from KVM's async page-fault worker today), brings the
  source page in if it isn't, takes a page of the tenant's arena, copies (or zeros, for zeros),
  and maps it writable there: the window's `ptes` entry names the private page, its mapping still
  the source's. KVM's worker's writes are resolved as reads, as now, and the vCPU's own access
  decides. On arm64, the write-protect fault reports the address, as now. *Done differently
  (TASK-164.1):* a private page is the VM process's own memory, a memfd laid out as the window
  (`dax::uffd::PrivateMemory`, `STAGE_PRIVATE`), not a page of the tenant's arena; `private_max`
  is reserved against the tenant's arena as the window is made (OPEN_QUESTIONS.md).
- **Private pages** are the device's: a source evicting its page skips them; a `MAP` or `UNMAP`
  over one frees it; the device's count and the tenant's are kept with them.
- **What goes:** open extents' sources in windows (`Source::is_open`, the open extent's memory in
  `crate::cache`), the commit's sealing of windows' mappings (`commit_seal_ns`, `make_read_only`),
  the open-page pin budget and its `ENOMEM` (`extentd/memory.rs`, `physmem::pin` for open extents;
  both stay, the tenants' limits and the reservation of `private_max`),
  asking guests to seal (`extentd/seals.rs`), writable `MAP`s, and `ExtentStore::open`'s mapped
  half (an extent open in memory, a VMO the window maps).
- **What stays:** the window, its second stage and the VMM's userfaultfd stage, `MAP_MANY`,
  `MAP_AHEAD`, sealed extents' shared pages, the pool, relief of clean pages, 159 for stores the
  window refuses.

## The guest

**Mapping (unchanged but for holes).** A file's chunk is a range of the window, page by page from
the sealed extents holding its data, by `MAP_MANY` and `MAP_AHEAD`; small files' chunks may be
slices of a sealed extent's region, read-only, as today. A range maps holes and the pages past the
file's end with `MAP_ZEROS`, so that every page of a range has a window page, and no write is new.
Reads, PMD read faults and readahead are as today.

**Writing.** A write takes a range: a write to a chunk a slice holds drops the slice and maps the
chunk as a range first (the slice's region may hold other files' blocks, and a clone's shared
block, which a store there would change for them). Then, for each page:

- `fxfs_dax_iomap_begin` with `IOMAP_WRITE` (a fault, `write(2)`, zeroing) adds the page to the
  file's **dirty set** if it isn't in it, reserving room for it in Fxfs (`fxfs_reserve_dirty`: a
  fault without room fails `SIGBUS`, a `write(2)` `ENOSPC`, as the page cache's do), counts it
  private if it wasn't, and returns `IOMAP_MAPPED` at the page's own window address. It holds the
  file's semaphore shared until `iomap_end`.
- A fault maps the page writable, and the guest's store makes it private on the host. A PMD write
  fault falls back, so that dirty pages are 4 KiB.
- A `write(2)` copies into the window. One covering more than a few pages not yet private sends
  `PRIVATE` for them first; fewer take a host fault each (~54 µs a page, nested, as open extents'
  first stores take today: docs/measurements, 2026-10-10T1953Z).
- No transaction, no move, no seal, no MAP but the range's own: a write is a copy into memory, as
  the page cache's is.

**Writeback** (`->writepages`: fsync, msync, sync, the flusher, the budget, freeze), per file,
the no-DAX path's (`fxfs_vfs_writepages`) with window pages for folios:

1. Under the file's semaphore shared: swap the dirty set out for an empty one (it becomes the
   **writeback set**), then `dax_writeback_mapping_range`, which write-protects the PTEs of every
   page dirtied by a fault. A store after the swap faults again, and adds its page to the new dirty
   set.
2. `fxfs_write_many` of the writeback set's pages, their bytes read from the window, with their
   reservations: Fxfs allocates the blocks now, copies the bytes into its transfer buffer, `WRITE`s
   them to new extents, and commits the transaction naming them after the bytes, as it does for the
   page cache. A store meanwhile may reach the copy or not, page by page, as without stable pages;
   its page is dirty again either way.
3. The writeback set is kept until the barrier after that transaction completes (the device's seal
   count passes the one noted): its pages' extents are sealed then.
4. A writeback that fails: what an I/O error stopped is clean, the error the file's, told by the
   next fsync and syncfs, as the page cache's; its private pages read as written until a rebind,
   a drop or a truncate shows what Fxfs holds. What Fxfs had no memory to write (`ENOMEM`) stays
   dirty, its room reserved again (`fxfs_reserve_dirty_what_fits`: the failed `fxfs_write_many`
   took it), and the writeback fails `ENOMEM` without setting the file's error: the next writes it.
   Clean, a rebind brought back the blocks those pages replaced and a clone carried them, though
   nothing had failed to be written (TASK-207). An eviction, after which no writeback finds the
   file, tries again for a second, then gives them up as an I/O error's.

**Rebinding** frees private pages: under the file's semaphore held exclusively, a private page in
neither set, whose data's extent is sealed, is `MAP`ped to that extent's page (`MAP_MANY`, a run
at a time): the same bytes at the same address, the host's private page freed, nothing in the guest
changed. By invariant 3 no store can be lost. A store after it faults, and copies again.

**The budget.** The guest counts private pages: a page counts from the `iomap_begin` that makes it
writable (whether or not a store comes), until a `MAP` or `UNMAP` over it. Past seven eighths of
`private_max` a worker writes back dirty files and rebinds clean private pages, least recently
dirtied first, to three quarters; a fault or `write(2)` that finds it at `private_max` less a slack
of a page a CPU waits for the worker, holding nothing, as `balance_dirty_pages` waits after a fault.
`PRIVATE` is sent only within it.

**Window reclaim** (least recently used ranges, as today) writes a range's dirty pages back first,
and takes none whose private pages' extents aren't sealed yet, sealing (`COMMIT`) first if that
leaves nothing to take. So a range never maps an open extent: its private pages hold that data
until it is sealed.

**Truncate and punch** drop the range's pages in the hole from both sets, releasing their
reservations, and `MAP_ZEROS` them (or `UNMAP` past the end); entries go under `invalidate_lock`
as today. **Clone** (`remap_file_range`) writes back the source's range and seals before cloning,
as XFS writes back both files first; the destination's pages in the range are dropped as by a punch,
and mapped again from the clone's extents.

**Freeze** writes everything back, commits durably (sealing), and rebinds every private page: the
snapshot's window holds extents and zeros only, and with a private page left the store refuses the
snapshot's point (above). Thaw changes nothing.

**`MAP_PRIVATE` stores** are the kernel's: an anonymous guest page, copied from the window
(`dax_fault_cow_page`). **Long-term pins** stay refused.

**Files created in a mount** are page-cache inodes while they stay in memory: an inode made by
create, mknod, `O_TMPFILE` or a symlink's target never gets `S_DAX`. Their reads and writes are the
no-DAX path's, which a device without a window uses today. Read in again, after eviction, an inode
is DAX, unless its page-cache writeback has written an extent no barrier has sealed yet: the guest
keeps the inode numbers whose writeback wrote since the last seal began, a set each completed
seal empties (seconds at most, the journal's flushes seal), and such an inode is read in to the page
cache again. So a range never meets an open extent of a file read in. An inode created, then
truncated and rewritten in place, stays in the page cache until evicted.

## Fxfs: what delayed allocation needs

Nothing new: the page cache's path already allocates at writeback (TASK-80).

- **Reservations at dirtying** (`fxfs_reserve_dirty`, `fxfs_reserve_dirty_what_fits`,
  `fxfs_release_dirty`): room for each dirty page and for writing it back, so writeback can't fail
  `ENOSPC`.
- **Allocation and commit at writeback** (`fxfs_write_many`, `write_dirty`): new blocks, the bytes
  written, then the transaction; a crash before the commit leaves the old blocks and frees the new
  (orphans), as for the page cache.
- **Checksums of the copy, not the source:** Fxfs must checksum the bytes it wrote, its transfer
  buffer, not the window's page a store may change meanwhile, as the page cache's folios already
  require. A guest check stores through a mapping during fsyncs and replays the journal.
- **Clone:** source written back and sealed first (above), so a clone shares only sealed blocks.
- **Goes:** `txn_map_write`, `txn_map_write_holding`, `txn_map_write_keeping`, `Kept`,
  `ExtentAccess` (and `fxfs_dax_access`: every mapped piece is sealed), `Allocator::restage` and the
  staging extent's restaging (packing small files' blocks into shared extents at writeback is
  unchanged), `set_check_kept`, fxfs-posix's `dax_map_write`, `dax_begin_write`/`DaxWrite`,
  `retrying_sealed`, fxfs-linux's `restage`. `extents_in` returns extents only. *As built:*
  `ExtentAccess` and `enum fxfs_dax_access` stay, `Sealed` and `Open` alone: `extents_in` returns
  each extent with whether it is still open, which the window may not map yet
  (crates/fxfs/src/object_store/data_object_handle/dax.rs).

## Crash consistency and the fsync contract (doc-12 changes)

Unchanged in substance: an acknowledged fsync is *ordered*: its pages' bytes `WRITE`n, then the
transaction, then its barrier. What changes: dirty data lives in private host pages, as the page
cache's lives in guest RAM, and every crash loses what wasn't written back (guest, VMM or extentd:
the private pages go with the device). The window plays no part in recovery. doc-12's "a store
through a mapping ... may reach the store before that, whole page by whole page" becomes: reaches
the store only by writeback (fsync, msync, sync, the flusher at ~30 s, the budget), each page as
writeback copied it. The line about a `write` failing with EIO leaving zeros after its blocks moved
goes: nothing moves, and a `write(2)` cut short leaves the bytes it copied, as on ext4.

## Per page or per file

| | Per page (chosen) | Per file (written file leaves DAX) |
|---|---|---|
| The 400 GB case | met: unwritten pages DAX; written pages private, at most `private_max`, the rest written back, sealed and shared again | not met: the whole file through the guest's page cache once opened for writing |
| Lines (estimates) | net about -10k: the same ~-11.5k, then +~800 guest, +~500 host | net about -11k: ~-11.5k, then +~300 guest for the switch |
| Kernel | stock fs/dax.c paths only; patches 0005, 0008, 0009 go | the same, plus a live `S_DAX` switch no in-tree filesystem makes |
| Protocol | `DAX` copies on write, +`private_max`, `MAP_ZEROS`, `PRIVATE`; -`DAX_WRITE`, `MAP_WRITE`, `SEAL`, `ENOMEM` | -`DAX_WRITE`, `MAP_WRITE`, `SEAL`, `ENOMEM` |
| New invariants | 4 (above), none involving a seal | 2: the switch's exclusion; a page-cache inode never maps the window |
| pgbench, DB made in the run | no-DAX's (created files: page cache) | no-DAX's |
| pgbench, DB from a base or fork | to measure; expected 0.9-1.0x no-DAX: a host fault the first store of each page | no-DAX's, after the switch |
| Base reads | today's DAX | today's DAX |
| fsync | no-DAX's: `WRITE`s, `COMMIT`; no window change | no-DAX's |
| First store to a page | guest fault, then host fault, ~50-110 µs nested | guest page-cache fault, after a `READ` if not cached |
| Memory, 400 GB case | guest: none for file data; host: ≤ `private_max`, shared pages besides | guest: the file's working set, a second copy of the host's |
| Memory, pgbench/sqlite on a base DB | DB read through DAX, shared; written pages ≤ `private_max` | DB in the guest's page cache, as no-DAX today (469 MB-class) |
| Risk | host fault cost nested; tuning `private_max` | the switch against mappings, pins and readers in flight; density of written files |

Per file is not much simpler: it removes the same writable machinery, and adds a switch the kernel
doesn't support in its place. Nor faster where it counts: both write through the no-DAX path's
writeback. With per file the user gives up: DAX for every page of a file once it is written, so
databases, key-value stores, model and pack files written a little read the rest through the guest's
page cache, a second copy in guest RAM, missing to a `READ`; the 400 GB case; and forks sharing a
written file's unchanged pages. **Recommended: per page.**

## What goes

*Done at stage 4, 2026-11-04 (batch 2026-11-04e, TASK-164.7 and 164.8), split four ways: the guest
kernel's dax.c, Fxfs, the host, and the tests, simulator, specs, scripts and docs.*

- **Guest, dax.c** (4,983 lines, an estimated half): the gate (`fxfs_dax_store_begin`, device.c's),
  seals' write-protects and forgetting writable pages (`fxfs_dax_seal`, `fxfs_dax_sealed`,
  `fxfs_dax_clear_writable`, `fxfs_dax_writable_pte`), moves (`fxfs_dax_map_moved`,
  `fxfs_dax_iomap_write`'s moves, `fxfs_dax_cow`), restaging and staging regions
  (`fxfs_dax_restage_work`, `fxfs_dax_region_open`, `fxfs_dax_grow*`, writable regions), write(2)
  through the staging region and lagging pages (`fxfs_dax_catch_up`, `fxfs_dax_lags`,
  `fxfs_dax_fall_behind`, `dax_writes_through`), held copies (`dax_fail_held`, `Kept::Held`), the
  57 sites using errors to break the fault/semaphore/gate/seal cycle (`-EXDEV` retries;
  `FXFS_DAX_MAX_DROPS` stays, bounding slices dropped), `fxfs_dax_begin_write`/`end_write`,
  `fxfs_dax_any_open`, the seal diagnostics (`dax_check_seals`, `dax_verify_seals`,
  `dax_check_moves`, the shadow's seal checks),
  and the fault injections for seals (`dax_fault_seals`, `dax_fault_seals_moved`).
- **Kernel patches:** 0005 (a shared fault replacing a PMD's entry), 0008 (splitting a PMD entry a
  shared PTE fault replaces) and 0009 (a write's copy from kernel memory not stopped) serve writes
  that move pages: dropped once guest/sim and the DAX suites pass without them. 0010 and 0011
  (shared reads of slices) stay. 0012 (invalidation under the entry lock) stays while anything
  invalidates a DAX mapping outside truncation. 0007 (why a fault failed) stays, until TASK-160.5.
- **Fxfs:** listed above.
- **Host:** listed above. `F_DAX_WRITE`, `F_SEAL`, `MAP_WRITE`, `seal_asked`/`SEALED` in
  vendor/firecracker's device and the driver.
- **Specs:** `specs/dax` (DaxSeal: seals against stores) and `specs/dax-region` (DaxRegion: writable
  regions) go; `specs/extent-open` loses its MAP half (an extent opened by a mapping), which leaves
  nothing a spec is needed for: it goes. `specs/follower` stays. New: `specs/dax-cow`, below.
- **guest/sim** (9,110 lines of C): stays as the harness, `fs/dax.c` and dax.c compiled as they are.
  Its Fxfs model loses open extents, staging, moves and seals' effects; its host gains copy-on-write
  (the memfd page by page, a store to a read-only page copying instead of SIGSEGV, past
  `private_max` SIGSEGV). Mutants of deleted code go (`bdc653701-fault-waits-gate`,
  `sealer-keeps-ptes-writable`, `task-121-no-map-moved`, `task-150.20-*`, `task-150.8-*`,
  `task-153.3-*`, ...); scenarios of moves and seals go (`load_during_move`,
  `open_extent_read_only`, `writes_through`, `sealed_piece_reach`); reads' stay.
- **Docs:** doc-4 (above), doc-9 (a note: superseded but for its section 3; a seal no longer touches
  a window), doc-12 (above), doc-2 (open decision 1 settled: writes copy on write in host memory,
  allocated at writeback), README's exits (159's causes). A dead end, `dead-ends/writable-dax.md`,
  for the whole approach.
- **Tasks:** TASK-160.2, 160.4, 160.5 are re-scoped to what's left of dax.c once this lands.

## What stays, and why

The read path whole: `MAP_MANY` (a chunk's pieces in one round trip), `MAP_AHEAD` (pages mapped
ahead of first accesses), PMD read faults, slices of sealed extents for small files, ranges
outliving inodes, LRU reclaim of ranges, the window pool, followers. They are why DAX reads are
3.8x faster than no-DAX's first reads and cost the guest no memory.

## A spec: `specs/dax-cow`

One page of one file, two tasks, the host: stores (fault and `write(2)`), writeback (swap,
write-protect, copy, commit, seal), rebinding, truncation, host copy-on-write. Invariants: an fsync
that returned has every store completed before it began in the extent its commit names; a rebind
never discards a store; nothing maps an open extent; private pages ≤ `private_max`. Mutations: the
swap after the write-protect; a rebind under the semaphore shared; a rebind before the seal;
writeback forgetting the writeback set before its barrier; `iomap_begin` not counting a page
private.

## Performance and density: what to expect, how to measure

- **Writes:** created files are exactly the no-DAX path: pgbench on a database made in the run
  should match `fxfs-guest-pgbench` (1,736 tps against DAX's 1,012). Files read in pay a host fault
  for each page's first store (~54-110 µs nested) and nothing after; fsync is the no-DAX path's,
  with no write-protect at the seal (58 µs of extentd's 126 a commit today) and no evacuation.
- **Measured, and accepted (the user, 2026-11-06):** pgbench on a fork of a base's database runs at
  0.836 and 0.854 of no DAX (GCE r650, TASK-164.10), not the 0.9-1.0 expected here: each page's first
  store in the fork is a `PRIVATE`, ~486 µs nested a request (the daemon 83 µs, the VM process's run
  of the pages 135, the request's way through the VMM the rest), and the database, 164k pages
  rewritten every ~5 minutes, is more than `private_max` keeps. The perf gate's `forked-dax` is 0.8
  since. What could cut it further, the VMM's request path and a page made private without
  replacing its mapping, is TASK-164.11's, for later.
- **Reads:** unchanged; files written in this mount read through the page cache until evicted.
- **Memory per VM:** guest page cache for created files (balloon free-page reporting returns what
  they free, Firecracker's balloon has it), private pages ≤ `private_max` on the host, shared
  sealed pages besides.
- **The gate** (scripts/perf-gate-gce.sh, pgbench-gate.py): add `fxfs-guest-pgbench-dax-forked`
  (pgbench over a database a base holds, so its files are DAX and every write copies on write);
  a 400 GB-shaped microbenchmark (a file in a base, 8-64 GiB, mapped shared, 5% of pages stored at
  random, msync: store faults' latency, private pages, writeback time, tps); first reads of a base
  (today's numbers); and a density gate: per VM, guest RSS of page cache, private pages, and shared
  pages mapped, with N VMs of one base.

## Stages, each landable on main with the workspace and guest suites passing

*Stages 1, 2 and 4 done, 2026-11-04; 3 skipped: the user had writable DAX deleted with no A/B
batch.*

1. **Host copy-on-write** (`DAX_COW`, `private_max`, `MAP_ZEROS`, `PRIVATE`), offered beside
   `DAX_WRITE`; a guest takes one. Tests: dax.rs units (a store to a read-only page copies once;
   zeros copy nothing; `MAP`/`UNMAP` free private pages; a source's eviction skips them; past
   `private_max` refused), Shuttle (a store against a `MAP` of the page, against eviction), the uffd
   stage on GCE (a vCPU's store copies, KVM's worker's write doesn't). doc-4 gains the feature.
2. **Guest copy-on-write behind `fxfs.dax_cow`** (a module parameter, default off), with created
   files in the page cache: dirty set, writeback from the window, rebinding, budget, holes as zeros,
   slices read-only, truncate/punch/clone/freeze. Tests: the DAX suites and the crash suites
   (`-crash-dax`, `-killed`, `-daemon-killed`) with it on; new guest checks: stores through a
   mapping during fsyncs (data and journal replay), a rebind forced each second under stores
   (`fxfs.dax_rebind_every`), truncate/punch/clone of dirty pages, the budget reached (throttling,
   no 159), `MAP_PRIVATE` stores, a freeze with private pages; guest/sim with the host's copy on
   write, new scenarios and the mutants above; `specs/dax-cow`. The perf gate with the new cases,
   judged by TASK-164's AC #2.
3. **The default:** `fxfs.dax_cow` on; the writable path kept one batch for an A/B on one host.
   Skipped.
4. **Deletion** of the writable path: the lists above, the parameter, `DAX_WRITE`, `SEAL`; the
   docs. Done: `DAX_COW` went too, `DAX` meaning copy on write; the gate's and suites' `-dax-cow`
   cases merged into their `-dax` ones.
5. **Measured, if the gate asks:** `PRIVATE` ahead of append streams of read-in files; rebinding
   at writeback for files written once; `private_max`'s default.

## Subtasks to create under TASK-164

*Created as TASK-164.1 to TASK-164.8, in this order.*

1. **Host: copy-on-write DAX window pages** (`DAX_COW`, `private_max`, `MAP_ZEROS`, `PRIVATE`).
   AC: a store to a sealed or zero page gets a private copy at the same address; `MAP`/`UNMAP` free
   it; eviction skips it; past `private_max` refused (159); KVM's async writes don't copy; doc-4
   says so; units, Shuttle, GCE pass.
2. **Guest: DAX writes copy on write, allocated at writeback** (behind `fxfs.dax_cow`). AC: dirty
   set with reservations; writeback by `fxfs_write_many` from the window, commit after bytes;
   rebinding under the semaphore held exclusively; the budget throttles under `private_max`; holes
   as zeros; writes through ranges only; truncate, punch, clone, freeze; DAX and crash suites pass
   with it on.
3. **Guest: files created in a mount are page-cache inodes until read in again.** AC: `S_DAX` set
   at read-in only; a created file read in after eviction is DAX, unless written since the last
   seal began; mixed inodes in one superblock pass the DAX and no-DAX suites.
4. **Tests: guest/sim's host copies on write; `specs/dax-cow`.** AC: the sim's host model, scenarios
   of rebind against stores, writeback against stores, truncate of dirty pages; each mutation and
   mutant listed above caught; zloop 3,000 seeds clean.
5. **Gate: pgbench over a base's database, the 400 GB-shaped microbenchmark, memory per VM.** AC:
   cases in perf-gate-gce.sh and pgbench-gate.py with thresholds; a run recorded in
   docs/measurements.
6. **Copy-on-write DAX by default.** AC: the dax-gate and perf gate pass; writes of created files at
   no-DAX's tps, base first reads at today's DAX's, the forked pgbench within the gate's threshold.
7. **Delete writable DAX.** AC: the guest, Fxfs, host, protocol, specs, sim, patches 0005/0008/0009
   lists done; `cargo nextest run --workspace` on macOS and GCE; `scripts/upstream-changes.py --check`.
8. **Docs: doc-2, doc-4, doc-9, doc-12, README, the dead end.** AC: each says what this design does,
   nothing of seals moving pages.

## Decisions taken here

- Per page, by host copy-on-write, over per file (the comparison above). Undoing it: per file, with
  the live switch; stages 1 and 2's host work unused.
- Created files in the page cache until read in again. Else every new file costs a range's `MAP`
  and a host fault or a `PRIVATE` per page, where the page cache costs a copy. Undoing it: the
  conditions in `fxfs_vfs_inode_init` and create; a pool of window space mapped as zeros and made
  private ahead would be needed instead.
- Writable DAX deleted outright at stage 4, no flag kept and no A/B batch run (the user,
  2026-11-04). Undoing it: revert batch 2026-11-04e, whose commits dead-ends/writable-dax.md names.
- `private_max` reserved against the tenant at open, not charged as pages come, so that a store
  never waits for memory and no VM asks another to seal. Its default (a quarter of the window is
  the proposal) waits on the density target, the review's open decision 5.
