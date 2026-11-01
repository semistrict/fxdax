# guest/sim: dax.c in a process of its own

`guest/linux/fs/fxfs/dax.c` runs here unmodified, with no VM, over a fake kernel, as OpenZFS's
libzpool runs its kernel code under ztest. A seed drives a scheduler whose scheduling points are the
kernel's locks, so a failure replays exactly, in milliseconds, under ASan and UBSan.

TASK-149.14's spike. Nothing here is built by the workspace, or run by a suite yet.

## Build and run

On Linux, with clang and lld, against a configured tree of the guest's kernel
(`scripts/build-guest-kernel.sh` leaves one):

```
make KERNEL=<tree> BUILD=<dir>
<dir>/tests/smoke                        # one task: writes, faults, seals, an evacuation
<dir>/tests/interleave SEED [OPERATIONS] # tasks at once, the seed's interleaving
KERNEL=<tree> BUILD=<dir> ./mutants.sh   # puts past bugs back: which the sim finds
```

`interleave`'s flags say what to log of a run; replaying its seed with them shows how it failed.

## What is real, and what is simulated

Compiled as they are, from the tree or the repo:

| Real | From |
| --- | --- |
| `dax.c`, `device.c`, `hist.h`, `vfs.h`, `fxfs.h`, `device.h` | fs/fxfs: all of the window's protocol and the gate |
| `fs/dax.c` | with guest/linux/patches/0005-0008: DAX entries, their locks, shared folios |
| `fs/iomap/iter.c`, `lib/xarray.c`, `lib/genalloc.c` | the kernel's |
| `linux/{list,rculist,hash,hashtable,xarray,iomap,dax,genalloc,wait,wait_bit}.h` | the kernel's headers |

Written here (see each file's head for what it leaves out):

| Simulated | Where | How far it goes |
| --- | --- | --- |
| Scheduler, locks, wait queues, RCU, workqueues | `kernel/sched.c`, `locks.c`, `workqueue.c` | One task at a time; wait queues as the kernel's; rwsem order as the kernel's, not its optimistic spinning |
| Page tables and faults | `kernel/mm.c` | Two flat arrays per VMA, with `mm/memory.c`'s insert and fault rules rewritten. No TLB, no MMU, no GUP, no private mappings |
| VFS and page cache | `kernel/vfs.c`, `mount.c` | What dax.c calls. `super.c` is not built: its truncate, punch and fsync are rewritten |
| Host device | `kernel/device.c`, `host.c` | The window is real memory, a memfd mapped page by page, so a store the host refuses is a SIGSEGV. Requests complete at once: no virtqueue, no second stage, no async page faults |
| Fxfs | `fxfs/fxfs.c` | A model of what the protocol assumes: blocks, open extents, the staging extent, `FreeAfter::Flush`, one seal in four a flush. No journal, tree or crash consistency |

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
livelocks; use after free (ASan); stores to sealed or unmapped window pages (SIGSEGV); a sleep in
an atomic section; every `WARN` and lockdep assertion in the real sources; bytes wrong through
read(2), a mapping, or Fxfs's extents.

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
- **The followers**, reflinks and private mappings, none of which the sim reaches yet.

## What it found

In dax.c as it stands, none confirmed on a real kernel yet:

1. **A write over a hole leaves a mapping reading zeros.** `tests/hole_mapped_then_written` fails,
   one task, no seal, 142 scheduling points. A write whose pieces aren't in a row in the window takes
   several iomaps, and only the first says its pages are new; the rest keep the zero page a load
   mapped. A write *from* that mapping then stores the zeros, for good. `fixes/hole-pieces.patch`.
2. **A load through a PTE reads zeros while a move loops.** A fault that a seal keeps from the gate
   moves its blocks again, and a flush may free the old extent, whose window page maps zeros, before
   the range follows. Needs a flush inside the fault: seed 234 at 1000 operations, with the fixes of
   1 and 3. `fixes/moved-before-mapped.patch`, which fixes 1 too.
3. **A shared PMD read fault WARNs at `fs/dax.c:458`.** `dax_folio_put` leaves a page's index in the
   union with its share count, so a shared association of a page that had an entry at a nonzero
   index skips `dax_folio_init`, and a PMD goes over an order 0 folio. dax.c's slices and moved pages
   are shared; needs a chunk one extent of 2 MiB exactly, at a PMD-aligned window address.
   `fixes/shared-pmd.patch`.

With the three fixes, 3,000 seeds at 200 operations and 700 at 1,000 pass. Likely false positives
of dax.c's diagnostics, each traced but left for someone to decide:

- `dax_shadow`, so off unless `-s`: an evacuation's read-only MAP over an own-space page once
  writable, which a move relocated, its PTEs gone with it.
- `dax_check_places` (seed 739 at 1000 operations): `i_size` read, then a lock that yields, then
  Fxfs asked, as a truncate shrinks the file between.

## Past bugs put back

`mutants.sh`, on top of the three fixes: 20 past bugs, picked by hand from dax.c's history for being
in its protocol, so not a fair sample of all its bugs.

| Past bug | Found at seed | How |
| --- | --- | --- |
| dce4f144b, only a fault forgets a move | 1 | SIGBUS |
| TASK-92.28, a move loses an append | 5 | WARN |
| TASK-124, holes not relocated | 6 | SIGBUS |
| TASK-128, prune ignores stale entries | 7 | `fs/dax.c` WARN |
| TASK-147, a write's own source shrinks its range | 7 | short write |
| TASK-121, moved blocks left unmapped | 13 | diagnostic WARN |
| TASK-79.50, non-atomic clear of writable bits | 60 | a split of a bitmap write |
| TASK-122, a page copied onto itself | 188 | bytes wrong in Fxfs's extents |
| bdc653701, a fault waits for a seal | 416 | deadlock |
| TASK-101, evacuation lets go of the semaphore | 74, at 1000 operations | diagnostic WARN |
| TASK-125, relocated pages stay mapped | 730, without fix 2, which masks it | bytes wrong |
| TASK-79.12, a fault waits for the semaphore | not in 2,000; 1508 and 2618 in earlier workloads | deadlock |
| TASK-79.104, evict doesn't wait for evacuation | no: since TASK-101 the hold is never seen | equivalent mutant |
| TASK-123, patch 0008, a PMD row broken up | no | PMD faults are rare: a targeted test is due |
| TASK-99, TASK-140, TASK-44.21 | no | need an open extent the model never reports sealed first, or reads racing it |
| TASK-133, every way out maps what moved | no | the workload doesn't take those exits |
| TASK-79.28, a range remapped too small | no | no other task faults a file growing, and TASK-147 narrowed it |

300 seeds reach 72% of dax.c's lines and 62% of `fs/dax.c`'s. Not reached: reclaim (the window
never fills), followers, reflinks and the failing paths of faults.

## Layout

- `shim/`: the kernel's headers, as `ALIASES` maps them to `shim/sim/*.h`, `-nostdinc`. Dax.c sees
  nothing else but `REAL_HEADERS`.
- `kernel/`: the fake kernel. `kernel/host.c` alone sees libc.
- `fxfs/`: the model of Fxfs.
- `tests/`: what runs. A test fails by panicking, the seed named.
- `mutants/`: past bugs, each a patch that takes its fix away; `fixes/`: candidate fixes of bugs
  found here, kept out of dax.c until someone decides on them, and which `mutants.sh -b` applies.
- `inventory.py`: what a source spells of the kernel, by libclang against the real headers.
