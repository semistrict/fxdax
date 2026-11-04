# guest/sim: dax.c in a process of its own

`guest/linux/fs/fxfs/dax.c` runs here unmodified, with no VM, over a fake kernel, as OpenZFS's
libzpool runs its kernel code under ztest. A seed drives a scheduler whose scheduling points are the
kernel's locks, so a failure replays exactly, in milliseconds, under ASan and UBSan.

TASK-149.14. The workspace doesn't build it: `zloop.sh` runs it, in CI's `guest-sim` job.

## Build and run

On Linux, with clang and lld, against a tree of the guest's kernel: `scripts/build-guest-kernel.sh`
leaves one, and `kernel-tree.sh <tree>` fetches what's needed of one, in seconds, with nothing built.

```
make KERNEL=<tree> BUILD=<dir>
<dir>/tests/smoke                        # one task: writes, faults, seals
<dir>/tests/interleave SEED [OPERATIONS] # tasks at once, the seed's interleaving
KERNEL=<tree> BUILD=<dir> ./zloop.sh     # with fixes/*.patch in: every test, then 3,000 seeds of each
KERNEL=<tree> BUILD=<dir> ./mutants.sh -b fixes/x.patch...   # puts past bugs back: which it finds
KERNEL=<tree> BUILD=<dir> ./mutants.sh --check                # that each still applies, in seconds
<dir>/tests/failures SEED [OPERATIONS]   # the same, as allocations, MAPs and UNMAPs fail
<dir>/tests/scenarios [NAME]             # paths no seed takes, each played out; none lists them
```

`interleave`'s flags say what to log of a run; replaying its seed with them shows how it failed. The
other tests are scenarios, one run each, some of them a guest check's steps, through `dax_pause`:
every file of `tests/` but those `lib.sh` lists as `seeded` (`interleave`, `failures`), so a new one
runs as it is added.

`zloop.sh` runs dax.c with the candidate fixes in, as bugs known aren't news. As dax.c takes a fix its
patch stops applying, which fails `zloop.sh` until the patch is removed. An open bug's failure,
as `known-failures` matches it, is counted apart and doesn't fail it, so that one bug doesn't fail
every run: its line goes with its fix. CI's `guest-sim` job runs seeds 1 to 3,000; the nightly hunt
(`.github/workflows/hunt.yml`) 20,000 more each night, from a first seed that moves, and keeps what
failed, with `replay`, how to run each again.

## What is real, and what is simulated

Compiled as they are, from the tree or the repo:

| Real | From |
| --- | --- |
| `dax.c`, `device.c`, `hist.h`, `vfs.h`, `fxfs.h`, `device.h` | fs/fxfs: all of the window's protocol and the gate |
| `fs/dax.c` | with guest/linux/patches/0005-0012: DAX entries, their locks, shared folios |
| `fs/iomap/iter.c`, `lib/xarray.c`, `lib/genalloc.c` | the kernel's |
| `linux/{list,rculist,hash,hashtable,xarray,iomap,dax,genalloc,wait,wait_bit}.h` | the kernel's headers |

Written here (see each file's head for what it leaves out):

| Simulated | Where | How far it goes |
| --- | --- | --- |
| Scheduler, locks, wait queues, RCU, workqueues | `kernel/sched.c`, `locks.c`, `workqueue.c` | One task at a time; wait queues as the kernel's; rwsem order as the kernel's, not its optimistic spinning |
| Page tables and faults | `kernel/mm.c` | Two flat arrays per VMA, with `mm/memory.c`'s insert and fault rules rewritten, and `mmap_lock`. No TLB, no MMU, no GUP, no private mappings |
| VFS and page cache | `kernel/vfs.c`, `mount.c` | What dax.c calls. `super.c` is not built: its resize, punch, clone and fsync are rewritten, so that a bug in them shows on a real kernel alone |
| Host device | `kernel/device.c`, `host.c` | The window is real memory, a memfd mapped page by page, so a store the host refuses is a SIGSEGV. Requests complete at once, or fail with EIO as a test asks, or with ENOMEM past the open pages a test lets it hold, a seed in four, or a MAP waits on its way as a test holds it: no virtqueue, no second stage, no async page faults. Copying on write (`DAX_COW`, doc-13), a seed in four of `interleave` and the `cow_` scenarios: `PRIVATE` copies pages to private memory at the same addresses, ENOMEM past `private_max`; `MAP_ZEROS`, and a MAP or UNMAP freeing private pages, as doc-13's host. A store to a sealed or zero page, which the host's userfaultfd write-protect would copy at, fails the run: the guest makes each page it dirties private first (TASK-164.9), so such a store is to a page no iomap made private. No KVM async page faults, whose writes the host takes as reads |
| Fxfs | `fxfs/fxfs.c` | A model of what the protocol assumes: blocks, open extents that go one way, shared blocks, the staging extent, `FreeAfter::Flush`, one seal in four a flush, transactions that fail, `zero_tail`. Writeback (`fxfs_write_many`) allocating then: its copy of the pages, a page at a time, a new extent, written, then the commit, which a test may hold to look at what a crash would leave; reservations of dirty pages, given back once. No journal, tree or crash consistency |

So a run tests dax.c against a second writing of its assumptions. Where the model differs from
Fxfs, or `kernel/mm.c` from `mm/memory.c`, a run proves nothing. Five of the first failures here
were this simulator's own bugs: read a failure as a question until it's reduced.

A seed replays exactly, down to its trace hash: the process runs itself again with ASLR off, as
`fs/dax.c` hashes pointers into its wait table. Logging that takes a lock changes the run: `-T`
(dax.c's trace, which reads the seal count under a spinlock) can make a failing seed pass. `-v`,
`-w`, `-W` and `-a` take none.

Linking the real Fxfs takes a userspace build of `guest/rust/fxfs-linux`'s `fxfs_*` over a fake
`fxfs_device_submit`. `crates/fxfs-posix` already runs on Linux, so the work is its C ABI and running
its executor on the scheduler's tasks, so that its awaits are scheduling points. Without that, its
sleeps happen off the seed, and a run stops replaying.

## What it can and can't find

It finds, by construction: orderings of tasks at lock points, device requests, `fxfs_dax_delay()`
and the sim's splits of non-atomic bitmap writes and kernel copies; deadlocks, with who holds what;
livelocks, and faults that go round 256 times; use after free (ASan); stores to sealed or unmapped
window pages (SIGSEGV), and a read-only MAP or UNMAP under a writable PTE, whether or not a store
comes; a sleep in an atomic section; every `WARN` and lockdep assertion in the real sources; bytes
wrong through read(2), a mapping, or Fxfs's extents, at every fsync as well as at the end. Copying
on write: a store to a page no `PRIVATE` made the VM's (dirty without private), a `PRIVATE` past
`private_max` (159 on a host), a writable MAP, a store a rebind or a writeback lost, an fsync missing a store that ended before it began, a punch or truncate a
writeback undoes, a crash's file other than old or new, and room for dirty pages given back twice
or kept. A MAP of an open extent fails EPERM, as the host's does, and is counted: the `cow_`
scenarios, whose Fxfs says open of every extent until its seal, expect none.

Every store carries a stamp, as ztest's blocks carry their tags: file, place, worker, generation, and
the seal it was made in. A task reads its own slots exactly, and any other's within bounds: no older
than the last store that ended, or anything a read saw, before it began, nor newer than the last
store begun as it ended, and zeros only if a store overlapped it. So a stale read, a zero read, data of
another file or place, and a read from the future each fail as what they are, on the read that
sees them, whichever task's it is; the log says which store the bytes came from, and how many seals
ago.

It can't find:

- **Weak memory.** Atomics are sequentially consistent. A missing barrier, or a `data_race`
  (KCSAN's findings, as GCE r255's), doesn't show.
- **Anything between two scheduling points.** A race on plain fields with no lock between its two
  sides isn't explored unless a delay point or a split falls there.
- **Real page tables.** TLB shootdowns, PTE races with hardware, `get_user_pages`, KVM's stage 2,
  async page faults, userfaultfd: all outside it.
- **The host.** extentd, uffd and the VMM are fakes here.
- **Fxfs.** Journal, compaction, crash recovery, and whatever the model gets wrong.
- **Lock order inversions that no seed plays out:** there is no lockdep graph.
- **The followers and private mappings**, which the sim doesn't reach yet.
- **Copy on write's host side**: userfaultfd, KVM's async page faults resolved as reads, a source
  evicting its pages, a snapshot refused while pages are private. The model copies pages at a
  `PRIVATE` alone, in the request, and nothing else of the host's runs meanwhile.

## What it found


TASK-150.8 to .14, a static audit's findings, each a scenario here, and `tests/failures` finds most
of them without one: a short copy and a write from its own pages losing what whole pages held; an
append cut short leaving the last block behind Fxfs; a double free; a sealed piece mapped writably
outside the gate; a failed UNMAP stopping unmount; a range left behind its blocks after failed MAPs;
writable bits a seal misses. And TASK-79.53 again, on DAX: a mapping's store past the end read back;
and a writer killed between its move and its copy, which left zeros.

In dax.c as it stood at TASK-149.14, first found here, the first since fixed:


1. **A write over a hole leaves a mapping reading zeros.** Confirmed on a real kernel, GCE r395 and
   r396, whose `write_over_mapped_hole` said `page 1 through the mapping is stale`. A write whose pieces
   aren't in a row in the window takes several iomaps, and only the first says its pages are new; the
   rest keep the zero page a load mapped. A write *from* that mapping then stores the zeros, for good.
   `tests/hole_mapped_then_written`, `fixes/hole-pieces.patch`, which goes once dax.c has the fix.
2. **A load through a PTE reads zeros while a move loops.** A fault that a seal keeps from the gate
   moves its blocks again, and a flush may free the old extent, whose window page maps zeros, before
   the range follows. `tests/load_during_move`, a guest check's steps,
   `fixes/moved-before-mapped.patch`, which fixes 1 too. Unconfirmed.
3. **A shared PMD read fault WARNs at `fs/dax.c:458`.** `dax_folio_put` leaves a page's index in the
   union with its share count, so a shared association of a page that had an entry at a nonzero index
   skips `dax_folio_init`, and a PMD goes over an order 0 folio. Slices say shared.
   `tests/pmd_over_stale_index`, a guest check's steps,
   `fixes/shared-pmd.patch`, a line in `fs/dax.c`. Unconfirmed.
4. **A writable MAP outside the gate.** A reflink has part of an open extent read as sealed, so a
   reader outside the gate holds a sealed piece of a writable region: `fxfs_dax_new_slice` reaches for
   it with no look at the region.
   `tests/sealed_piece_reach`, one task, whose guest check needs a lockdep kernel; seed 13464 at 1,000
   operations without the fix. `fixes/sealed-reach-locked.patch`. Unconfirmed.

6. **`fs/dax.c:416` WARN: a write puts a PMD's folio that a mapping still maps.** A write says its
   pages are new, or shared, and has `dax_iomap_iter` invalidate their entries, a PMD's over the row:
   `invalidate_inode_pages2_range` puts each entry's folio, and unmaps the range only after, with
   nothing keeping faults out meanwhile. A PMD's folio that a PMD, or a PTE through its entry, maps
   is split under it. Before fafefbac7 the write said a page that moved shared, as a read fault
   mapped the row; since, a write over a page a failed move left a hole says it new. At 200
   operations 3 seeds in 30,000 on main (106b6128d). `tests/pmd_entry_invalidated_mapped`, a fault's
   move that a seal leaves in an extent the device doesn't hold, then a PTE of the PMD's folio,
   then the write. Fixed in `fs/dax.c` (patches/0012, TASK-150.21): each entry's pages are unmapped
   under its lock before its folio goes, as `grab_mapping_entry` does a PMD it downgrades.

TASK-153.3's, all three on main before it too: a slice's refusal said `-ESTALE` to its readers, which
drop it once: two refusals, SIGBUS (seed 1102; main's seed 3762 at 1,000 operations). A write through a
region left a fault's PTE of its page where its blocks were, mapped between the move's unmap and the
look at `users`: a stale read (seed 7289 at 200). And `failures` kept window from use, for good, until
none was left (now a quarter of it at most).

At 106b6128d merged: 3,000 seeds at the mix, 3,000 at 1,000 operations and 30,000 at 200 fail at the
WARN of 6 alone, 5 times, all at 200.

With patch 0012 (TASK-150.21): `zloop.sh`'s 3,000 seeds and every test of one run pass.

## Past bugs put back

`mutants.sh`: past bugs, picked by hand from dax.c's history for being in its protocol, so not a
fair sample of all its bugs. 3,000 seeds at most, an eighth at 1,000 operations. Seeds move as the
workload does.

| Past bug | Found at | How |
| --- | --- | --- |
| patch 0008, a PMD row broken up | seed 1 | SIGBUS, in a second mapping's fault of a page |
| TASK-121, moved blocks left unmapped | seed 8 | diagnostic WARN |
| TASK-133, every way out maps what moved | seed 16 | a zero read, with Fxfs failing a transaction |
| TASK-92.28, a move loses an append | seed 28 | diagnostic WARN |
| TASK-147, a write's own source shrinks its range | seed 32 | short write |
| TASK-79.50, non-atomic clear of writable bits | seed 40 | a split of a bitmap write |
| bdc653701, a fault waits for a seal | `pmd_entry_invalidated_mapped`; seed 72 before patch 0012 | a hang: 120 s, no log |
| TASK-99, a region opens under a read-only MAP | seed 2544; the hunt, at 56669 and 62739 of 43,001 to 63,000 | a store the host refused, as the VM stopped. The look at `mapped` goes back with what made a reader and a writer meet: the model's device write reads as opening its extent |
| TASK-153.3, a device write reopens its extent, in the model, as the Rust had it | seed 2544 | `fxfs_dax_region_open`'s WARN: an open extent's region mapped read-only |
| TASK-153.3, a slice's refusal says `-ESTALE` to its readers | no, in 3,000, nor 500 at 1,000 operations, nor the hunt's 20,000 | found by `interleave 1102 1000` before 106b6128d; main at 106b6128d fails so at seed 3762, 1,000 operations |
| TASK-153.3, a write through a region keeps a fault's PTE of the page it lags | no, in 3,000, nor seeds 3,001 to 15,000 at 200 operations | a stale read, `interleave 7289 200`, before patch 0012 moved the seeds; then by the mix, 6's WARN came first, at seed 1486. TASK-149.26 |
| TASK-79.12, a fault waits for the semaphore | seed 2865 | its deadlock: no task can run. Before patch 0012, `fs/dax.c:416`'s WARN came first, at seed 1692 |
| TASK-140, a read-only slice of an open extent | `open_extent_read_only` | a store the host refused: shared blocks of an open extent slice its region, as they did, and `region_open` leaves such a region be |
| TASK-150.20, a MAP the host has no memory for makes no room | `open_memory_out` | a write's EIO; and every seed that limits the host's memory, a seed in four, in `interleave` and `failures` alike |
| TASK-150.20, each way in alone: a write, a fault, zeroing, a read | `open_memory_out`, 8, 8, 592 | a write's EIO, a fault's EFAULT, a punch's ENOMEM, and a read's failure: reads reach it least |
| TASK-44.21, a sealed piece reached for | `open_extent_read_only` | `fxfs_dax_region_open`'s WARN: shared blocks of an open extent go through its region, as a slice's, the one way since fafefbac7 |
| TASK-44.19, `fxfs_dax_map_to` without the region's lock | `reaches_at_once`; no, in 3,000 | bytes wrong: two slices reach at once, the nearer MAP ends last and says the region maps less, and a seal gives the window past it back under the further slice, which reads the next region's page |
| patch 0012, an invalidation puts a folio mapped still | `pmd_entry_invalidated_mapped` | `fs/dax.c:416`'s WARN: a PMD's folio split under a PTE of it |
| TASK-79.28, a range remapped too small | no | equivalent since TASK-147: a fault sizing a range from the old end holds the invalidate lock `fxfs_dax_fit` waits for, and one after sees `growing`. `task-79.28-before-147`, with that gone too, shows at seed 31 |

Before patch 0012, the `fs/dax.c:416` WARN wasn't these alone: it showed with no mutant (6, above),
about a seed in 6,000 at 200 operations, and came first in TASK-79.12's and the lag's.

300 seeds of the first workload reached 72% of dax.c's lines and 62% of `fs/dax.c`'s; not measured
since. Eviction, reflinks, reclaim (a run in four has a window its files fill) and Fxfs's failures are
reached now. Followers and failures of faults past those aren't.

And by `tests/scenarios`, each at once, with how many seeds of `tests/failures` find it alone:

| Bug | `failures` | How |
| --- | --- | --- |
| TASK-150.8, whole pages moved before the write has its bytes | 1 | bytes wrong |
| TASK-150.10, `maps` and `own` apart, freed twice | 1 | ASan |
| TASK-150.11, shared blocks through a region, by either way in | not run | a store the host refused, at once, in `open_extent_read_only` |
| TASK-150.12, the pool destroyed with what an UNMAP failed for | 1 | `BUG_ON` |
| TASK-150.13, a range behind its blocks left in use | no, in 400 | a scenario: its extent has to die |
| TASK-150.14, writable bits of a file not listed | no, in 2,676 | not found: its scenario took a slice of a clone's blocks, which shared blocks no longer make. `fxfs_dax_set_writable` keeps it unbuildable by hand, not by a test |
| TASK-79.53, a mapping's store past the end read back | 2 | bytes wrong |
| A write past the end that copies nothing grows the file | not run | a scenario |
| TASK-150.20, writes past the host's memory for open extents | 200 of 200 that limit it | the write's EIO, as generic/129's |

150 seeds of `interleave` and `failures` with the scenarios reach 73% of dax.c's lines and 60% of
`fs/dax.c`'s. Not reached: reclaim (the window never fills), followers, and Fxfs failing.

## Copy-on-write DAX

TASK-164.4, doc-13. A seed in four of `interleave` (or `-C`) runs copying on write, with a
`private_max` of 256 pages or a quarter of the window; and these scenarios, at 64 pages:

| Scenario | What it checks |
| --- | --- |
| `cow_rebind_against_stores` | stores through a mapping and by write(2) to some pages, each checked to read its last before the next, as writes to twice `private_max` more have the budget's worker write back and rebind: no rebind loses a store |
| `cow_writeback_against_stores` | each of 40 fsyncs, once it returns, finds in Fxfs every page's round from before it began, stores going on |
| `cow_truncate_dirty` | a truncate through dirty and clean private pages frees them, zeros the last page's tail, gives their room back; grown again, the file reads zeros there |
| `cow_punch_dirty` | punches of dirty pages, a syncer writing back and a storer beside them, leave holes: no writeback undoes one |
| `cow_crash_between_bytes_and_commit` | writeback's commit held after its bytes: Fxfs shows the file as it was, the commit would show what was copied; a store meanwhile reaches the fsync's writeback for its size |
| `cow_swap_during_write` | a write(2) paused between the iomap that dirtied its page and its copy (`dax_pause`): the fsync's swap waits for it |
| `cow_rebind_during_writeback` | a rebind (`dax_rebind_now`) while writeback's commit is held waits for it (`wb_lock`) |
| `cow_rebind_skips_dirty` | the worker's writeback held, a page dirtied again: its rebind leaves that page, and the PTEs faults made writable are write-protected |
| `cow_rebind_against_write` | a rebind's MAP held on its way: a write(2) to a page it found clean waits for it |
| `cow_commit_failed` | `dax_fail_commits`: the fsync fails EIO, Fxfs holds the file as it was, the next fsync succeeds |
| `cow_reclaim_during_writeback` | a writeback paused after its swap (`dax_pause`), before it reads its pages from the window, as other files' chunks fill the ranges' half of it: reclaim takes their ranges, not the one the writeback reads (GCE r544's class: a range's space freed and given to another while something reads it there) |
| `cow_writeback_during_fault` | a store's fault paused past its iomap_end (`dax_pause`), its PTE writable and its DAX entry held: the worker's writeback write-protects the PTE all the same, so that its rebind's read-only MAP finds none writable, and the store faults again and dirties its page |
| `cow_fsync_during_writeback` | a writeback paused after its swap: a second fsync, and then a sync of the filesystem, nothing dirty, wait for it, and find every store before them in Fxfs as they return |
| `cow_truncate_held` | a truncate's MAP of zeros over 48 dirty private pages held on its way: a second file's write waits for the budget until it ends, the host never holding more than `private_max` |
| `cow_writeback_before_store` | a store's fault paused past its iomap_end, its page dirty, its store not come: an fsync writes it back, and the seals after free its old extent; the store, let go, keeps the rest of the page as it was, not the zeros the freed extent reads, as its iomap made it private (TASK-164.9) |
| `cow_map_during_seal` | an fsync's seal held after it began (`fxfs_sim_hold_seals`), Fxfs taking its writeback's extent for sealed already, the device not: a read's slice, and then a write's range, of the chunk wait for the seal to map it, and ask no MAP the host refuses |
| `cow_reclaim_after_writeback_met_a_fault` | an fsync writes back a page whose DAX entry a load's fault, paused, holds, which stays tagged dirty: once other files' chunks fill the window, reclaim has taken the range all the same |
| `cow_private_failed` | the device failing the `PRIVATE` of a write(2)'s iomap, and of a store's fault's: the write fails EIO, the store SIGBUS, nothing dirty nor reserved; again, both succeed |

Each ends with read(2) and Fxfs holding every page's last store, no room for dirty pages left
reserved, and no MAP of an open extent asked. The rules' mutants:

| Mutant | Caught by |
| --- | --- |
| `task-164.4-swap-shared` | `cow_swap_during_write` |
| `task-164.4-writeback-unlocked` | `cow_rebind_during_writeback` |
| `task-164.4-wb-sync-none` | `cow_rebind_against_stores`: a rebind's read-only MAP under a writable PTE; `cow_rebind_skips_dirty`; `cow_writeback_against_stores` |
| `task-164.4-protect-by-entries` | `cow_writeback_during_fault`: the rebind's read-only MAP under the fault's writable PTE |
| `task-164.4-fsync-skips-writeback` | `cow_fsync_during_writeback`: the second fsync returns, Fxfs holding the stores' rounds before |
| `task-164.4-sync-skips-writeback` | `cow_fsync_during_writeback`: the sync returns so |
| `task-164.4-forget-before-free` | `cow_truncate_held`: the second file's write fails, its `PRIVATE` refused past `private_max` |
| `task-164.4-writeback-unowned` | the first store of any `cow_` scenario, `cow_rebind_against_stores`'s: to a page no `PRIVATE` made the VM's. Before the host refused it, `cow_writeback_before_store`: the rest of page 3 read zeros |
| `task-164.4-extents-during-seal` | `cow_map_during_seal`'s write: a MAP of an extent the host has open |
| `task-164.4-slice-during-seal` | `cow_map_during_seal`'s read: the same, of a slice's region |
| `task-164.4-release-stale-entries` | `cow_reclaim_after_writeback_met_a_fault`: the range is never reclaimed |
| `task-164.4-rebind-shared` | `fxfs_dax_cow_map`'s lockdep assertion, in any rebind; `cow_rebind_against_write` |
| `task-164.4-rebind-dirty` | `cow_rebind_against_stores`: a rebind's read-only MAP under a dirty page's writable PTE; `cow_rebind_skips_dirty` |
| `task-164.4-commit-failure-dropped` | `cow_commit_failed` |
| `task-164.4-private-uncounted` | `cow_rebind_against_stores`: a write fails, its `PRIVATE` refused past `private_max` |
| `task-164.4-truncate-unwritten` | expected equivalent: a grow drops the chunk at the old end, which writes it back |
| `task-164.9-private-failure-ignored` | `cow_private_failed`: the write whose `PRIVATE` failed stores to a page no `PRIVATE` made the VM's |
| `task-164.2-writeback-holds-no-range` | `cow_reclaim_during_writeback`: the writeback hands Fxfs the zeros of the chunk reclaim gave its range's space to, reads the window where nothing is mapped, or the range reclaim freed (ASan) |

In dax.c's copying on write as it stood at batch 2026-11-04c (CI run 41823852629), which 612 of
`interleave`'s 750 copy-on-write seeds failed, first found here, all six since fixed:

1. **Writeback left a fault's PTE writable.** It write-protected the pages it took by
   `dax_writeback_mapping_range`, by the DAX entries; a write fault past its iomap_end has let the
   file's semaphore go, and holds its page's entry until it returns, and `dax_writeback_one`, finding
   the entry locked mid-walk, gives up on it (`get_next_unlocked_entry` says it gone). The rebind
   then mapped the page read-only under the writable PTE, which the host refuses a store through
   (`a read-only MAP ... maps writably`, 15 seeds); without a rebind, a store through it after the
   writeback's copy is in no set. Fixed by write-protecting the set's pages by the page tables, as
   the sealer does (`fxfs_dax_cow_protect`). `cow_writeback_during_fault`, seed 22.
2. **An fsync returned before a writeback under way committed.** `fxfs_dax_cow_write_back` returned
   at once if nothing was dirty, without the writeback lock: the budget's writeback had swapped the
   pages out, and had yet to write and commit them. Fixed by looking under the lock. A sync's
   `fxfs_dax_settle` passed such a file by, as it had nothing dirty, the same way: it writes back
   every file with private pages now. `cow_fsync_during_writeback`, seed 962.
3. **Private pages counted free before the host freed them.** `fxfs_dax_cow_forget` ran before the
   UNMAP of a range, or a truncate's MAP of zeros, past which the budget gave their room to another
   store, refused past `private_max` (`the window refused an access`). Fixed by counting them free
   once the request ends, and never if it failed: the host holds them for good then.
   `cow_truncate_held`, seed 90.
4. **A range a writeback cleaned could never be reclaimed.** 1's other half: the DAX entry a fault
   held through the writeback stays tagged dirty, and with no page dirty no writeback cleans it
   again; `fxfs_dax_release`'s invalidation then failed for ever, so that reclaim passed the range
   by, and a fault short of window reclaimed it again and again (`livelock: a fault had its range
   dropped 257 times`; a drop, which deletes entries whatever their tags, got by). Fixed by cleaning
   a clean range's entries as it is released. `cow_reclaim_after_writeback_met_a_fault`, seed 3390
   at 1,000 operations, one of 2,000.
5. **A writeback took pages the host hadn't made private.** An iomap marks its pages dirty before
   their stores, which make them private on the host; a fault's store comes after the writeback that
   write-protected its PTE (1), so that the writeback copied the window page while it still mapped
   the old block, moved the block, and Fxfs freed the old extent at the flush after: the store then
   made its page private from the zeros the window reads there, and the next writeback wrote them
   (`a zero read` of another worker's slot). Fixed by making the dirty set private (PRIVATE) before
   it is swapped out (`fxfs_dax_cow_own`). `cow_writeback_before_store`, seed 2802 at 1,000
   operations.
6. **A range or a slice asked to MAP an extent a seal in progress had yet to seal.** Fxfs says an
   extent minted before a seal is sealed from the seal's beginning (fxfs-linux's `Seal::begin_with`
   calls `OpenExtents::seal` before the COMMIT), which the host refuses to map (-EPERM): dax.c
   recovered, sealing and trying again or dropping the slice, but asked, against doc-13's
   invariant 1. No seed of `interleave` checks for it; mutants that moved `cow_punch_dirty`'s
   schedule met it there. Fixed by looking the extents up again once the seal has ended
   (`fxfs_dax_cow_extents`, and in `fxfs_dax_new_slice`, -EBUSY for a fault). `cow_map_during_seal`.

And five of this simulator's own: the syncer checked, after an fsync of one file, every file's
slots in Fxfs, which copying on write holds only once each is written back (516 of the 612, the
"zero reads", and 64 "stale reads"); `fxfs.dax_fail_maps` fails a read's range's MAP too, copying on
write, which `interleave` took for a bug (`read_file` and `user_load` try again as it runs out); a
failed write's resync compared read(2), which sees the dirty pages, with Fxfs before writing them
back; the model of Fxfs sealed an extent minted during a seal at its COMMIT, but called it open
until a scheduling point after, through which a truncate's `move_tail` wrote the sealed extent
(seed 32826); and `sim_file_evict` evicted an inode reclaim held (`igrab`), which
the kernel can't, so that reclaim released a range through the old inode's mapping while the new
one's faults mapped it (`fs/dax.c:462`'s WARN, seed 222 at 1,000 operations): eviction now waits
for the last reference, and `igrab` fails meanwhile.

After the fixes: `zloop.sh -s 3000` passes (every test and scenario, 3,000 seeds of `interleave`
and `failures`); and copying on write, seeds 2 to 43,000 of `interleave` that copy (10,750) at
200 operations, the 4,000 of them to 16,000 at 1,000, and 2,000 others with `-C`, all pass.

### One owner of a page's state (TASK-164.9)

1, 4 and 5 were one class: a page's state lived in three places changing at different moments,
dax.c's dirty and private bits, the kernel's DAX entry (its dirty tag and lock), and the host's
private page, and each bug was code trusting one to say what another held. Since TASK-164.9 the
bits are the state, each changed by one function, one transition each: clean to dirty and private
by the iomap that dirties a page (`fxfs_dax_cow_dirty`), whose `PRIVATE` has the host make it the
VM's own before any store can land, so that dirty implies private by construction; dirty to written
back by writeback alone (`fxfs_dax_cow_write_back`), from dax.c's dirty set, write-protecting by the
page tables; written back and sealed to rebound by a rebind's MAP (`fxfs_dax_cow_rebind`,
`fxfs_dax_cow_rebound`); and gone by an UNMAP or a truncate's MAP of zeros (`fxfs_dax_cow_forget`).
`fxfs_dax_cow_own`, and `fxfs.dax_private_ahead`, went: every iomap that makes a page private sends
its `PRIVATE`. The kernel's dirty tags are cleaned for the kernel's sake alone
(`fxfs_dax_cow_clean_entries`), by writepages, so that the inode is clean to it, and by a range's
release, under the invalidate_lock, so that invalidation takes its entries; no transition reads
them, nor waits on an entry's lock. A swap WARNs on a dirty page not private, and a rebind's MAP on
a dirty page.

And this host refuses a store to a page no `PRIVATE` made the VM's, where doc-13's would copy it:
the oracle of the rule, at the store, whichever path dirtied the page without it. The three bugs'
mutants, written again against the new code, stay caught: `task-164.4-protect-by-entries` (the
page tables' write-protect replaced by the entries' cleaning) by `cow_writeback_during_fault`;
`task-164.4-release-stale-entries` by `cow_reclaim_after_writeback_met_a_fault`, now that only
writepages and a release clean tags; and `task-164.4-writeback-unowned`, which no writeback can
reintroduce, writeback making nothing private any more, written as the iomap leaving the page to
its store, by the first store of any `cow_` scenario. `cow_private_failed` and
`task-164.9-private-failure-ignored` check that a `PRIVATE` that fails fails its write.

After it: `zloop.sh -s 3000` passes; copying on write forced (`-C`), seeds 1 to 20,000 of
`interleave` at 200 operations and 1 to 5,000 at 1,000, all pass; every cow mutant is caught but
`task-164.4-truncate-unwritten`, not in 2,000 seeds, as expected.

Doc-13's "a rebind before the seal" and "writeback forgetting its set before the barrier" have no
code to mutate: a rebind maps only what Fxfs says is sealed, and leaves a page private if the host
says EPERM (`fxfs_dax_cow_map`). `specs/dax-cow` models them.

## Layout

- `shim/`: the kernel's headers, as `ALIASES` maps them to `shim/sim/*.h`, `-nostdinc`. Dax.c sees
  nothing else but `REAL_HEADERS`.
- `kernel/`: the fake kernel. `kernel/host.c` alone sees libc.
- `fxfs/`: the model of Fxfs.
- `tests/`: what runs: `interleave`'s seeds, and scenarios, one run each. A test fails by panicking,
  the seed named.
- `mutants/`: past bugs, each a patch that takes its fix away; `fixes/`: candidate fixes of bugs
  found here, kept out of dax.c until someone decides on them, and which `mutants.sh -b` applies.
  Each applies with no fuzz, or fails `mutants.sh --check`, which CI runs.
- `known-failures`: open bugs' failures, which `zloop.sh` doesn't fail for.
- `inventory.py`: what a source spells of the kernel, by libclang against the real headers.
- `zloop.sh`, `check.sh`, `mutants.sh`, `lib.sh`, what they share, and `kernel-tree.sh`, the
  sources for them.
