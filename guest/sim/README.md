# guest/sim: dax.c in a process of its own

`guest/linux/fs/fxfs/dax.c` runs here unmodified, with no VM, over a fake kernel, as OpenZFS's
libzpool runs its kernel code under ztest. A seed drives a scheduler whose scheduling points are the
kernel's locks, so a failure replays exactly, in milliseconds, under ASan and UBSan.

TASK-149.14. The workspace doesn't build it: `zloop.sh` runs it, in CI's `guest-sim` job.

## Build and run

On Linux, with clang and lld, against a tree of the guest's kernel: `scripts/build-guest-kernel.sh`
leaves one, and `kernel-tree.sh <tree>` fetches what's needed of one, in seconds, with nothing built.
Elsewhere, `linux.sh COMMAND` runs any of what follows in a container (Docker, `Dockerfile`'s
image), with `KERNEL` and `BUILD` in its volume `fxdax-sim`: `./linux.sh ./zloop.sh`.

```
make KERNEL=<tree> BUILD=<dir>
<dir>/tests/smoke                        # one task: writes, faults, writeback, seals
<dir>/tests/interleave SEED [OPERATIONS] # tasks at once, the seed's interleaving
KERNEL=<tree> BUILD=<dir> ./zloop.sh     # with fixes/*.patch in: every test, then 3,000 seeds of each
KERNEL=<tree> BUILD=<dir> ./mutants.sh   # puts past bugs back (../mutants), each before its killer
KERNEL=<tree> BUILD=<dir> ./mutants.sh --check                # that each still applies, in seconds
<dir>/tests/failures SEED [OPERATIONS]   # the same, as allocations, MAPs, UNMAPs and PRIVATEs fail
<dir>/tests/scenarios [NAME]             # paths no seed takes, each played out; none lists them
KERNEL=<tree> BUILD=<dir> ./sweep.sh [RUN...] # each run of one, each of its failable calls failing in turn
SIM_SWEEP=N <dir>/tests/scenarios NAME    # one of the sweep's runs, its Nth call failing
```

`interleave`'s flags say what to log of a run, and `failures`' (`-v`, `-W`, `-a`, `-w`); replaying
its seed with them shows how it failed. The
other tests are scenarios, one run each, some of them a guest check's steps, through `dax_pause`:
every file of `tests/` but those `lib.sh` lists as `seeded` (`interleave`, `failures`), so a new one
runs as it is added.

`zloop.sh` runs dax.c with the candidate fixes in, as bugs known aren't news. As dax.c takes a fix its
patch stops applying, which fails `zloop.sh` until the patch is removed. An open bug's failure,
as `known-failures` matches it, is counted apart and doesn't fail it, so that one bug doesn't fail
every run: its line goes with its fix. CI's `guest-sim` job runs seeds 1 to 3,000 when what it tests
changed since its base (dax.c and the kernel's fxfs, the kernel's patches and build, this directory:
`scripts/guest-sim-scope.py`), else 1 to 200; the nightly workflow runs 3,000 every day; the nightly hunt
(`.github/workflows/hunt.yml`) 20,000 more each night, from a first seed that moves, and keeps what
failed, with `replay`, how to run each again.

## What is real, and what is simulated

Compiled as they are, from the tree or the repo:

| Real | From |
| --- | --- |
| `dax.c`, `pages.c`, `device.c`, `hist.h`, `vfs.h`, `fxfs.h`, `device.h` | fs/fxfs: all of the window's protocol |
| `data.c` | fs/fxfs: a file's changes as a whole, resize, punch (fallocate) and clone, with dax.c's operations (TASK-160.2) |
| `fs/dax.c` | with guest/linux/patches/0007 and 0010-0012: DAX entries, their locks, shared folios |
| `fs/iomap/iter.c`, `lib/xarray.c`, `lib/genalloc.c` | the kernel's |
| `linux/{list,rculist,hash,hashtable,xarray,iomap,dax,genalloc,wait,wait_bit}.h` | the kernel's headers |

Written here (see each file's head for what it leaves out):

| Simulated | Where | How far it goes |
| --- | --- | --- |
| Scheduler, locks, wait queues, RCU, workqueues | `kernel/sched.c`, `locks.c`, `workqueue.c` | One task at a time; wait queues as the kernel's; rwsem order as the kernel's, not its optimistic spinning |
| Page tables and faults | `kernel/mm.c` | Two flat arrays per VMA, with `mm/memory.c`'s insert and fault rules rewritten, and `mmap_lock`. A pin, as `get_user_pages` takes one (`sim_pin_user_page`), is the page's reference alone, faulted in as the kernel faults. No TLB, no MMU, no private mappings |
| VFS and page cache | `kernel/vfs.c`, `mount.c` | What dax.c and data.c call; the VFS's checks before a clone, and its writing both back, as `generic_remap_file_range_prep`. `super.c` is not built: its fsync, and its writing of an inode's attributes, its size alone here, are rewritten, so that a bug in them shows on a real kernel alone |
| Host device | `kernel/device.c`, `host.c` | The window copies on write (doc-13): real memory, a memfd mapped page by page, read-only, so that a store the host refuses is a SIGSEGV. A MAP maps sealed extents alone (EPERM for an open one, which tests count) and zeros past what was written; `MAP_ZEROS` maps zeros; `PRIVATE` copies pages to private memory at the same addresses, ENOMEM past `private_max` (a quarter of the window, or what a test sets); a MAP or UNMAP frees private pages, as doc-13's host. A store to a sealed or zero page, which the host's userfaultfd write-protect would copy at, fails the run: the guest makes each page it dirties private first (TASK-164.9), so such a store is to a page no iomap made private. Requests complete at once, or fail with EIO as a test asks, or a MAP waits on its way as a test holds it: no virtqueue, no second stage, no async page faults, whose writes the host takes as reads |
| Fxfs | `fxfs/fxfs.c` | A model of what the protocol assumes: blocks, open extents that go one way, shared blocks, the staging extent `zero_tail` moves a last block to, `FreeAfter::Flush`, one seal in four a flush, reservations that fail. Writeback (`fxfs_write_many`) allocating then: its copy of the pages, a page at a time, blocks of the staging extent for 256 KiB at most, as Fxfs packs small files, else a new extent, written, then the commit, which a test may hold to look at what a crash would leave, or fail; reservations of dirty pages, given back once. No seal comes between taking blocks and writing them, so the packed write a seal refuses, which Fxfs tries again in an extent of its own, has no counterpart. No journal, tree or crash consistency |

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
livelocks, and faults that go round 256 times; use after free (ASan); stores to unmapped window
pages, and to pages no `PRIVATE` made the VM's (dirty without private) (SIGSEGV); a MAP or UNMAP
under a writable PTE, whether or not a store comes; a `PRIVATE` past `private_max` (159 on a
host); a sleep in an atomic section; every `WARN` and lockdep assertion in the real sources; bytes
wrong through read(2), a mapping, or Fxfs's extents once written back, at every fsync as well as at
the end; a store a rebind or a writeback lost, an fsync missing a store that ended before it
began, a punch or truncate a writeback undoes, a crash's file other than old or new, and room for
dirty pages given back twice or kept; a load faulting again on a page its mapping held, which nothing
but a fault unmapped (`kernel/mm.c`, each mapping's pages held as a fault maps them and let go as a
rebind's MAP, an UNMAP, an eviction, a truncate or a punch unmaps them, a fault's unmapping of its
own page from the others holding them still: read faults undoing each other's mappings, a fault
each, as TASK-152's livelock, which succeed, and nothing else checks; TASK-149.14). A MAP of an open
extent fails EPERM, as the host's does, and is counted: the `cow_` scenarios, whose Fxfs says open of every extent until its seal, expect none.

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
- **Real page tables.** TLB shootdowns, PTE races with hardware, `get_user_pages`' own races
  (its fast walk, a pin taken as a mapping goes), KVM's stage 2, async page faults, userfaultfd:
  all outside it. A pin is a reference a test takes and drops.
- **The host.** extentd, uffd and the VMM are fakes here.
- **Fxfs.** Journal, compaction, crash recovery, and whatever the model gets wrong that
  `crates/fxfs-sim-diff`, which runs the model beside Fxfs, doesn't find ("The model against Fxfs").
- **Lock order inversions that no seed plays out:** there is no lockdep graph.
- **The followers and private mappings**, which the sim doesn't reach yet.
- **Copy on write's host side**: userfaultfd, KVM's async page faults resolved as reads, a source
  evicting its pages, a snapshot refused while pages are private. The model copies pages at a
  `PRIVATE` alone, in the request, and nothing else of the host's runs meanwhile.

## The sweep, and the end of a run (TASK-149.15, TASK-149.18)

`tests/failures` fails calls at random, one in so many; the sweep enumerates. Every call of the code
under test that a real kernel may see fail asks `kernel/sweep.c` first: allocations by fs/fxfs,
fs/dax.c and the XArrays and pool they use; the device's MAPs, UNMAPs and PRIVATEs; and the model of
Fxfs's room for dirty pages, growths, writeback's commits, clones (part way, as Fxfs's transactions
of one), seals and lookups. `SIM_SWEEP=N` fails the Nth, once; up to it the run is the run without
it. `sweep.sh` runs each run of one (every test of one run, every scenario; read_pressure with two
readers of two rounds) as it is, to count its calls, then once for each N; `-s K/N` every Nth of each
run's calls alone, from the (K mod N)+1th, as CI's `guest-sim-sweep` does with K its run's number
when what it tests is unchanged, the nightly workflow sweeping them all. Once the call has failed,
what the test expects is void (`sim_expect`, `sim_unexpected`: logged, and the run goes on); what holds
whatever fails isn't: ASan, UBSan, LeakSanitizer (every run, at exit), the WARNs of dax.c and
fs/dax.c, this kernel's checks, and the end of the run. A run still going 300 s of virtual time after
the failure, every task asleep, one at a pause point or a hold its test can no longer let go of, ends
there, void; with none, a deadlock or a livelock stops it, as without a failure.

Every test ends with `sim_end`: the mount, if one is left, synced (an error once, a sweep's failure
once, then it must succeed), each file fsynced whose writeback error no fsync told yet, read whole
as Fxfs's extents hold it and as long as Fxfs says (but where a writeback failed: the window then holds
what Fxfs doesn't, as a page cache does), its known bytes durable; then unmounted, as an unmount does:
mappings gone, files evicted, the volume removed, the window mapping nothing, and holding no private
page, but what UNMAPs that failed kept. Then how many calls the run reached, for the sweep.

The file model (`kernel/shadow.c`) is what tasks were told: every byte a write(2), a store, a
truncate, a punch or a clone said a file holds, once the call ended, unless another call changed its
page meanwhile, or the call failed, or its bytes were a task's memory. A read(2) or a load reads it;
an fsync that returned 0, and the end's sync, find it in Fxfs's extents, and the size: every
acknowledged write durable. A writeback that fails, as it fails, forgets what changed since the file
was last durable: its pages are clean, and a rebind or a drop may show what Fxfs holds at once.

Every waiter is woken (`kernel/sched.c`): a timed wait whose time ends it, its entry queued, ends
early, as a spurious wake, and fails the run if it then finds its condition true: a wake lost, which a
time limit would have hidden. No task ends with an entry queued.

In dax.c, found by the sweep and the file model, each fixed:

1. **An fsync returned without a range dirtied before it began.** Writeback counted its file's dirty
   ranges with the semaphore shared, allocated its batch, and swapped no more than that many with it
   held for writing: a store between dirtied the first chunk's range, which the swap took, passing the
   second's by. `failures 62`, the file model at the syncer's fsync. Counted again under the write lock,
   the batch made larger if need be. `cow_writeback_counts_again` (pause point 10, `COUNTED`),
   `task-149.15-writeback-batch-short`.
2. **A sync without the memory for its list of files returned 0**, having written none back
   (`fxfs_dax_busy_files`): -ENOMEM now. The sweep of `short_copy`, the end's sync, the file model.
   `task-149.15-sync-without-memory`.
3. **A write that could never have room in the window waited for ever.** Each round of the budget's
   worker, which each wait for room asked for, counted as a range let go of, and reset the wait's count
   toward giving up (`FXFS_DAX_NO_ROOM_WAITS`). read_pressure's window, its aligned room taken by a
   failed UNMAP: livelock. The worker's writeback lets go of its ranges as it ends, which says so.
   `unmaps_failed_take_the_window`, `task-149.15-round-releases`.
4. **A chunk's range had no room once failed UNMAPs took a piece of every aligned slot** (TASK-149.33),
   most of the window free: read_pressure's 8 MiB window, after one failed UNMAP, failed every chunk's
   write ENOSPC. With nothing to reclaim, a chunk's range takes room off a chunk's alignment, mapped a
   page at a time (`window_unaligned` counts them). `unmaps_failed_leave_room_unaligned`,
   `task-149.33-chunks-only-aligned`.

And this simulator's own: a failed mount (the device, the DAX, the volume) went on with what it hadn't;
a test's struct files and the model's extents' counts leaked; ended tasks' stacks were kept until a
run ended, gigabytes of a work's queued again and again; a sync's recorded error was told twice.

| Mutant | Caught by |
| --- | --- |
| `task-149.15-writeback-batch-short` | `cow_writeback_counts_again`: the second chunk's write not in Fxfs after the fsync |
| `task-149.15-sync-without-memory` | `sweep scenarios short_copy`: the end's sync, its allocation failed, writes nothing back |
| `task-149.15-round-releases` | `unmaps_failed_take_the_window`: livelock, the write never failing ENOSPC |
| `task-149.33-chunks-only-aligned` | `unmaps_failed_leave_room_unaligned`: the write fails ENOSPC, the window's room unaligned |
| `task-149.18-round-unwoken`, the worker's round waking no one it throttled | `interleave` seed 5: a lost wake, the throttled write's |
| `task-149.18-release-unwoken`, a range let go of waking no one waiting for room | `read_pressure`: a lost wake, the writer's |

## What it found

Copy-on-write DAX's bugs, the DAX there is since writable DAX was deleted (2026-11-04,
dead-ends/writable-dax.md), are in "Copying on write" below.

### Writable DAX's, before 2026-11-04

Its scenarios and mutants went with it (`load_during_move`, `open_extent_read_only`,
`sealed_piece_reach`, `writes_through`, `pmd_entry_invalidated_mapped`, the moves', seals', regions'
and lagging pages' scenarios and mutants), but for those of what stays: `hole_mapped_then_written`
(holes now map zeros, which a write makes private), `pmd_over_stale_index`, the short copies, a
write from its own pages, and failed allocations and UNMAPs. What they found, as it was then:

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
fair sample of all its bugs. They are `../mutants`, one set with those only a real kernel finds
(TASK-160.32), each patch's header naming its killer, the run here that must fail with it in
(`../mutants/README.md`): `mutants.sh` runs that alone, a seeded one's seeds, 2,000 by default, an
eighth at 1,000 operations, until one fails, and fails if it doesn't. A hunt of everything with a
mutant in is `zloop.sh -p`'s. Seeds move as the workload does. Writable DAX's mutants went with it;
those of what stays, as they were found before copying on write, and since (batch 2026-11-04e,
every run copying on write, 2,000 seeds):

| Past bug | Found at | How |
| --- | --- | --- |
| TASK-147, a write's own source shrinks its range | seed 32; copying on write, seed 10 | short write |
| TASK-153.1, a fault with no limit on its range's drops | with TASK-140's bug, gone with writable DAX, a livelock; copying on write, no | equivalent: nothing else drops a fault's slice for ever |
| TASK-44.19, `fxfs_dax_region_map` without the region's lock | `reaches_at_once`, which wrote through slices, gone; no, in 3,000; copying on write, no | bytes wrong then: two slices reach at once, and the region says it maps less than it does. Copying on write, only reads take slices, and each range MAPs every page it uses: what is left is the region's put UNMAPping less than the host maps, which no oracle here checks |
| patch 0010, a read fault of a page others map, its blocks shared, replaces its entry (TASK-152) | `interleave` seed 197, each worker mapping the shared files itself | a load faults again, its mapping holding the page, a fault having unmapped it (`patch-0010-reads-replace-their-entries`) |
| patch 0012, an invalidation puts a folio mapped still | `pmd_entry_invalidated_mapped`, gone; copying on write, no | `fs/dax.c:416`'s WARN. Equivalent copying on write: no write says its pages are new, which invalidates, and a range's release unmaps (`dax_layout_busy_page_range`) under the invalidate lock before it invalidates |
| TASK-79.28, a range remapped too small | no | equivalent since TASK-147: a fault sizing a range from the old end holds the invalidate lock `fxfs_dax_fit` waits for, and one after sees `growing`. `task-79.28-before-147`, with that gone too, showed at seed 31 |

And by `tests/scenarios`, each at once, with how many seeds of `tests/failures` found it alone,
before copying on write:

| Bug | `failures` | How |
| --- | --- | --- |
| TASK-150.8, whole pages moved before the write has its bytes | 1 | bytes wrong: `short_copy`, `same_page` |
| TASK-150.10, `maps` and `own` apart, freed twice | 1 | ASan: `allocations_failed` |
| TASK-150.12, the pool destroyed with what an UNMAP failed for | 1 | `BUG_ON`: `unmap_failed` (`task-150.12-pool-kept`) |
| TASK-79.53, a mapping's store past the end read back | 2 | bytes wrong (`task-79.53-gap-left`) |
| A write past the end that copies nothing grows the file | not run | `write_past_end_copies_nothing` (`write-past-end-grows`) |

## Copying on write

TASK-164.4, doc-13. Every run copies on write, `interleave`'s with a `private_max` of 256 pages or
a quarter of the window, the other tests' a quarter; and these scenarios, at 64 pages:

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
| `cow_rebind_against_write` | a rebind's MAP held on its way: a write(2) to a page it found clean waits for it, and so does a store's fault, and a load's, each blocking on the file's semaphore holding its DAX entry |
| `cow_commit_failed` | `dax_fail_commits`: the fsync fails EIO, Fxfs holds the file as it was, the next fsync succeeds |
| `cow_reclaim_during_writeback` | a writeback paused after its swap (`dax_pause`), before it reads its pages from the window, as other files' chunks fill the ranges' half of it: reclaim takes their ranges, not the one the writeback reads (GCE r544's class: a range's space freed and given to another while something reads it there) |
| `cow_writeback_during_fault` | a store's fault paused past its iomap_end (`dax_pause`), its PTE writable and its DAX entry held: the worker's writeback write-protects the PTE all the same, so that its rebind's read-only MAP finds none writable, and the store faults again and dirties its page |
| `cow_fsync_during_writeback` | a writeback paused after its swap: a second fsync, and then a sync of the filesystem, nothing dirty, wait for it, and find every store before them in Fxfs as they return |
| `cow_truncate_held` | a truncate's MAP of zeros over 48 dirty private pages held on its way: a second file's write waits for the budget until it ends, the host never holding more than `private_max` |
| `cow_writeback_before_store` | a store's fault paused past its iomap_end, its page dirty, its store not come: an fsync writes it back, and the seals after free its old extent; the store, let go, keeps the rest of the page as it was, not the zeros the freed extent reads, as its iomap made it private (TASK-164.9) |
| `cow_map_during_seal` | an fsync's seal held after it began (`fxfs_sim_hold_seals`), Fxfs taking its writeback's extent for sealed already, the device not: a read's slice, a write's range, a load's fault and a store's, of the chunk, wait for the seal to map it, the faults holding their DAX entries, and ask no MAP the host refuses |
| `cow_reclaim_after_writeback_met_a_fault` | an fsync writes back a page whose DAX entry a load's fault, paused, holds, which stays tagged dirty: once other files' chunks fill the window, reclaim has taken the range all the same |
| `cow_private_failed` | the device failing the `PRIVATE` of a write(2)'s iomap, and of a store's fault's: the write fails EIO, the store SIGBUS, nothing dirty nor reserved; again, both succeed |
| `cow_store_to_pinned_slice` | a pin of a slice's page, as vmsplice's (`sim_pin_user_page`): a store to it, which can't use the slice, waits for the pin to go, its fault from user mode holding none, then lands (TASK-79.49; the guest check `slice_held`'s steps, gone with writable DAX) |

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
| `task-164.4-extents-during-seal` | `cow_map_during_seal`: a MAP of an extent the host has open (`fxfs_dax_sealed_extents`, which ranges and slices both look extents up by) |
| `task-164.4-slice-during-seal` | `cow_map_during_seal`'s read: the same, of a slice's region |
| `task-164.4-release-stale-entries` | `cow_reclaim_after_writeback_met_a_fault`: the range is never reclaimed |
| `task-164.4-rebind-shared` | `fxfs_dax_map_pages`' lockdep assertion, in any rebind; `cow_rebind_against_write` |
| `task-164.4-rebind-dirty` | `cow_rebind_against_stores`: `pages.c`'s WARN of a rebind's MAP over dirty pages; before TASK-160.4, a rebind's read-only MAP under a dirty page's writable PTE; `cow_rebind_skips_dirty` |
| `task-164.4-commit-failure-dropped` | `cow_commit_failed` |
| `task-164.4-private-uncounted` | `cow_rebind_against_stores`: a write fails, its `PRIVATE` refused past `private_max` |
| `task-164.4-truncate-unwritten` | expected equivalent: a grow drops the chunk at the old end, which writes it back |
| `task-164.9-private-failure-ignored` | `cow_private_failed`: the write whose `PRIVATE` failed stores to a page no `PRIVATE` made the VM's |
| `task-164.2-writeback-holds-no-range` | `cow_reclaim_during_writeback`: the writeback hands Fxfs the zeros of the chunk reclaim gave its range's space to, reads the window where nothing is mapped, or the range reclaim freed (ASan) |
| `task-79.49-store-fails-on-pins` | `cow_store_to_pinned_slice`: the store fails (SIGBUS), its fault's drop of the slice refusing to wait for the pin (TASK-149.31) |
| `unmapped-page-read-as-hole` | `hole_mapped_then_written`: a load of page 2, which a truncate's failed MAP of zeros left with no window page, reads the kernel's zero page where read(2) reads a write's bytes |

In dax.c's copying on write as it stood at batch 2026-11-04c (CI run 41823852629), behind
`fxfs.dax_cow` then, which 612 of `interleave`'s 750 copy-on-write seeds failed, first found here,
all six since fixed (the functions as dax.c names them since writable DAX went):

1. **Writeback left a fault's PTE writable.** It write-protected the pages it took by
   `dax_writeback_mapping_range`, by the DAX entries; a write fault past its iomap_end has let the
   file's semaphore go, and holds its page's entry until it returns, and `dax_writeback_one`, finding
   the entry locked mid-walk, gives up on it (`get_next_unlocked_entry` says it gone). The rebind
   then mapped the page read-only under the writable PTE, which the host refuses a store through
   (`a read-only MAP ... maps writably`, 15 seeds); without a rebind, a store through it after the
   writeback's copy is in no set. Fixed by write-protecting the set's pages by the page tables, as
   the sealer does (`fxfs_dax_protect`). `cow_writeback_during_fault`, seed 22.
2. **An fsync returned before a writeback under way committed.** `fxfs_dax_write_back` returned
   at once if nothing was dirty, without the writeback lock: the budget's writeback had swapped the
   pages out, and had yet to write and commit them. Fixed by looking under the lock. A sync's
   `fxfs_dax_settle` passed such a file by, as it had nothing dirty, the same way: it writes back
   every file with private pages now. `cow_fsync_during_writeback`, seed 962.
3. **Private pages counted free before the host freed them.** `fxfs_dax_forget` ran before the
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
   (`fxfs_dax_sealed_extents`, and in `fxfs_dax_new_slice`, -EBUSY for a fault). `cow_map_during_seal`.

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

After the fixes: `zloop.sh -s 3000` passed (every test and scenario, 3,000 seeds of `interleave`
and `failures`); and copying on write, seeds 2 to 43,000 of `interleave` that copied (10,750) at
200 operations, the 4,000 of them to 16,000 at 1,000, and 2,000 others forced to, all passed.

### One owner of a page's state (TASK-164.9)

1, 4 and 5 were one class: a page's state lived in three places changing at different moments,
dax.c's dirty and private bits, the kernel's DAX entry (its dirty tag and lock), and the host's
private page, and each bug was code trusting one to say what another held. Since TASK-164.9 the
bits are the state, each changed by one function, one transition each: clean to dirty and private
by the iomap that dirties a page (`fxfs_dax_dirty`), whose `PRIVATE` has the host make it the
VM's own before any store can land, so that dirty implies private by construction; dirty to written
back by writeback alone (`fxfs_dax_write_back`), from dax.c's dirty set, write-protecting by the
page tables; written back and sealed to rebound by a rebind's MAP (`fxfs_dax_rebind`); and gone
by an UNMAP or a truncate's MAP of zeros (`fxfs_dax_forget`, `fxfs_dax_dropped` since).
`fxfs_dax_cow_own`, and `fxfs.dax_private_ahead`, went: every iomap that makes a page private sends
its `PRIVATE`. The kernel's dirty tags are cleaned for the kernel's sake alone
(`fxfs_dax_clean_entries`), by writepages, so that the inode is clean to it, and by a range's
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

Since TASK-160.4 the bits aren't fields of a range any more, but a module's, `pages.c`, which
dax.c sees through its transitions alone, a function each: `fill` (a range's own MAP, or a slice's
region), `dirty` (the iomap), `swap` (writeback), `rebind` (a rebind's MAP), `cut` (a truncate's MAP
of zeros) and `unmap`, and queries. Each says what it changed, which dax.c counts, and checks the
rules at the end: dirty implies private, and private and zeros imply mapped. The mutants of page
state, written again against it, are caught as before (`task-164.4-forget-before-free`,
`-rebind-dirty`, `-writeback-unowned`, `-swap-shared`, `-private-uncounted`,
`task-164.9-private-failure-ignored`, `unmapped-page-read-as-hole`).

After TASK-164.9: `zloop.sh -s 3000` passed; copying on write forced, seeds 1 to 20,000 of `interleave`
at 200 operations and 1 to 5,000 at 1,000, all passed; every cow mutant was caught but
`task-164.4-truncate-unwritten`, not in 2,000 seeds, as expected.

Since writable DAX was deleted (2026-11-04) every run copies on write, `failures` and the one-run
tests too, which never had. CI's first run of it (41876989384, batch 2026-11-04e) failed 646 of
`failures`' 3,000 seeds, one bug of dax.c and one of this simulator's:

1. **A page of the file with no window page was read as a hole.** A truncate whose MAP of zeros
   past the new end fails leaves those pages unmapped (`fxfs_dax_resize_end`), for a write to map
   again; a write past them grows the file over them. A load of one then had its read's iomap say
   a hole, so that the fault mapped the kernel's zero page, and the write(2) after mapped the page
   and copied into it, with no iomap new (`IOMAP_F_NEW`) to take the zero page away: the load's PTE
   read zeros where read(2) read the write's bytes, and a write from the mapping copied them (seed
   1669, `reading it all ... byte 20480 is 0, not 0xa1`). Fixed by mapping such a page before any
   iomap of it, a read's as a write's (`fxfs_dax_map_missing`): a writable window's file has no
   holes the kernel sees. `hole_mapped_then_written`'s second half, `unmapped-page-read-as-hole`.
2. The syncer of `failures` took any fsync failing for a bug (645 seeds, `PANIC in syncer: fsync`):
   copying on write, an fsync writes back first, whose batch allocation fails ENOMEM as allocations
   are made to, the pages left dirty for the next. It takes ENOMEM now if an allocation failed
   meanwhile.

The model of Fxfs packs small writebacks into the staging extent since, as Fxfs does. After both:
`zloop.sh -s 3000` passed, with the model packing and without; every mutant was caught but five,
none in 2,000 seeds: `task-164.4-truncate-unwritten`, `task-79.28-range-too-small`, `task-153.1-fault-limit`
and `patch-0012-invalidate-unmaps-after`, equivalent as said above, and `task-44.19-map-to-unlocked`,
which no oracle here can see copying on write.

Doc-13's "a rebind before the seal" and "writeback forgetting its set before the barrier" have no
code to mutate: a rebind maps only what Fxfs says is sealed, and leaves a page private if the host
says EPERM (`fxfs_dax_map_pages`). `specs/dax-cow` models them.

## A file's changes as a whole (TASK-160.2)

Resize, punch and clone were super.c's, which isn't built here, and were written again in
`kernel/mount.c`, where they drifted from it: no size rolled back when Fxfs refused a growth, the
size of the file after it passed to `fxfs_dax_resize_end` as asked rather than as it was, no
rounding of a clone to blocks, no recovery of what a failed clone reached (TASK-150.9, 153.4 and
79.53 were bugs of the same seam). They are `data.c`'s now, over each file's `fxfs_data_ops`, the
page cache's (super.c) or the window's (dax.c), and this builds `data.c` as it is: a truncate is
`fxfs_data_resize`, a punch the file's `fallocate`, a clone its `remap_file_range`, as the VFS calls
them. The model of Fxfs takes `write_inode`, and fails a growth as `interleave`'s failing seeds ask
(`fxfs_sim_fail`), as it fails room for dirty pages; a cut never fails, Fxfs's borrowing the room.

| Mutant | Caught by |
| --- | --- |
| `task-160.2-resize-no-rollback` | `interleave`'s failing seeds: a growth that failed leaves the file as long as it was, seed 1, seed 8 since the model zeroes every growth's tail (TASK-149.14.2). Without that check, the end's check of Fxfs's size, seed 233 |

## The model's growths, as Fxfs's (TASK-149.14.2)

A DAX file's truncate keeps its last block (`FXFS_SET_KEEP_TAIL`), the window zeroing past the end
in its own page. Fxfs zeroes what a cut left there as the file grows, every way it grows, unless the
growth writes that block anew (`txn_grow_size`, `zero_tail_before_growth`), and a write of part of a
block keeps of the rest what the file held up to its end, zeros past it (`read_unaligned_ends`):
fxfs-posix's `every_growth_finds_what_a_cut_left_past_the_end_zeroed`. The model zeroed only a
truncate's growth: a writeback of the last page up to the end kept the bytes past it, and a writeback
past the end, or a clone, grew the file over them. Found by the hunt's range on main (f5a5a9d82),
`failures 140272 1000`: a writeback that swapped the last page out during a write past the end,
before the write grew the file, then the fsync's writeback past it; read(2) had zeros, Fxfs's
extents what the truncate left (`read(2) has 0, and Fxfs's extents 0x41`). The model's growths go
through one function now (`grow`, `zero_before_growth`), as Fxfs's do.

| Mutant | Caught by |
| --- | --- |
| `task-149.14.2-growth-keeps-tail` | `fxfs_growths_zero_past_a_cut`: a write past it |
| `task-149.14.2-partial-write-keeps-past-end` | `fxfs_growths_zero_past_a_cut`: a write in the block |

## The model against Fxfs (TASK-149.29)

`crates/fxfs-sim-diff` builds `fxfs/fxfs.c` as it is, over a shim of this kernel of its own (no
kernel tree, so on macOS too), and runs it beside Fxfs, fxfs-posix over its fake device, on random
sequences of what dax.c and data.c call of it: writebacks of whole pages and of parts, up to the
file's size, paid with dirty pages' room or not, and failing between their bytes and their commits;
truncates' sizes, cuts with `FXFS_SET_KEEP_TAIL` and without, and growths; clones, of blocks and to
the source's end; punches; seals; and remounts of Fxfs. After each, every file's size and bytes
agree, and its blocks, whole, through the extents, past the end too, as the window maps them; holes
and shared blocks are alike; and no extent Fxfs says open does the model say sealed, which would
have dax.c here MAP what a host refuses. Some sequences run an operation at one of the model's
`sim_point`s, where its locks let one in, as this simulator's tasks interleave: a seal's COMMIT and
end, writeback's copy, write, reply and commit. A workspace crate, CI's workspace part runs it, on
Linux and macOS, 64 sequences in 15 seconds on a quiet laptop; `FXDAX_SIM_DIFF_CASES` asks for
more, `FXDAX_SIM_DIFF_OPS` longer ones. Its `mutants.sh` runs it against each mutant of
`../mutants` that changes the model; those only it kills say `Kernel: model` and
`Killer: fxfs-sim-diff TEST`.

First found by it, both fixed:

1. **The model moved a file's last block at every growth past an end within it**, and at every cut
   that doesn't keep the tail, where Fxfs's `txn_zero_tail` leaves a block holding zeros past the
   end as it is. Bytes alike, but the model's block went to a new extent, open, unshared, where
   Fxfs's stays sealed and shared: dax.c here never met a growth that leaves its last block mapped.
   `task-149.29-tail-moved-though-zero`.
2. **Fxfs's: a writeback paid with its dirty pages' room failed NoSpace** on a filesystem all but
   empty, growing a file past an end within a block a cut had kept (`FXFS_SET_KEEP_TAIL`, a DAX
   file's): the growth writes that block again, zeroed past the end, which the room reserved for
   the pages didn't count. fxfs-posix's
   `a_writeback_paid_with_dirty_room_has_room_for_the_tail_its_growth_zeroes`.

| Mutant | Caught by fxfs-sim-diff | By this simulator (`mutants.sh`, 2,000 seeds) |
| --- | --- | --- |
| `task-149.14.2-growth-keeps-tail` | a write past a kept tail: bytes from the old end read back the cut's | `fxfs_growths_zero_past_a_cut` |
| `task-149.14.2-partial-write-keeps-past-end` | a write in a kept tail's block, past the end: the same | `fxfs_growths_zero_past_a_cut` |
| `task-164.4-model-seal-forgets-late` | a cut at a seal's end (`sim_point`): its block in an extent the model says sealed, Fxfs's open, where dax.c here would MAP it | no: `interleave -C` seed 32826 found it once |
| `task-149.29-tail-moved-though-zero` | a growth past a tail of zeros, its block shared by a clone: the model's no longer shared | no: dax.c is right either way |

Each by `the_models_past_drifts_from_fxfs`, a sequence each, in seconds, and by CI's 64 random
sequences alone: the second and third once the generator aimed writes at the last block past the
end, and an operation at a seal's points at a cut near the end, which 64 didn't find before.

## Layout

- `shim/`: the kernel's headers, as `ALIASES` maps them to `shim/sim/*.h`, `-nostdinc`. Dax.c sees
  nothing else but `REAL_HEADERS`.
- `kernel/`: the fake kernel. `kernel/host.c` alone sees libc.
- `fxfs/`: the model of Fxfs.
- `kernel/sweep.c`: what fails, and when (the sweep); `kernel/shadow.c`: the file model.
- `tests/`: what runs: `interleave`'s seeds, and scenarios, one run each. A test fails by panicking,
  the seed named.
- `../mutants/`: past bugs, each a patch that takes its fix away, its header naming its killer;
  `fixes/`: candidate fixes of bugs found here, kept out of dax.c until someone decides on them,
  which `zloop.sh` applies, and `mutants.sh -b` if asked. Each applies with no fuzz, or fails
  `mutants.sh --check`, which CI runs.
- `known-failures`: open bugs' failures, which `zloop.sh` doesn't fail for.
- `inventory.py`: what a source spells of the kernel, by libclang against the real headers.
- `zloop.sh`, `check.sh`, `mutants.sh`, `sweep.sh`, `lib.sh`, what they share, `kernel-tree.sh`, the
  sources for them, and `linux.sh`, which runs them elsewhere than on Linux.
