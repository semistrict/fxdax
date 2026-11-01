# The typecheck hang is a DAX entry lock leaked, or a wakeup lost

**Tried:** 2026-11-01, TASK-152, at fc281d8.
**Idea:** tsgo's threads sat in D state in get_next_unlocked_entry, none of them seen holding the
entry, so the lock looked leaked, or its waiters unwoken: patch 0008's split of a PMD entry changes
an entry's order under waiters whose wait queue hashes on that order, and a WAKE_NEXT consumed by a
task that doesn't pass it on ends the chain.
**Result:** reading fs/dax.c and patch 0008 found no such path, and guest/sim's read_pressure, the
real dax.c and fs/dax.c, 600 seeds, no deadlock. The sandbox's capture then showed no hang at all:
73,000 faults a second, 25 of 38 threads running, the entry's holder in unmap_mapping_pages.
**Why it's dead:** it was a livelock. Read faults said IOMAP_F_SHARED (8af34848c), so each unmapped
the page from the others, which faulted it back. A stack sample of tasks queued on a hot entry looks
like a lock never released. A simulator with no cost model can't show it either, as every fault
ends; counting faults per load did, 128 in 64 turns.
**Reopen if:** tasks stay in get_next_unlocked_entry with dax_faults_hist flat.
