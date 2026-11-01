# DAX overwrites holding the inode's lock shared, to end writes' wait for it

**Tried:** 2026-11-01, TASK-146.2, fxfs.dax_write_shared, r374 against r364, async-commit pgbench-dax; uncommitted, the patch not kept: a few lines in fxfs_dax_write_iter.
**Idea:** A write spent 45% of its time, 114 of 251 us, waiting for the inode's lock, behind other writes and behind reads waiting for the file's semaphore.
**Result:** The wait went, 114 -> 1 us. tps didn't: 2498/2495/2507/2539/2597 (mean 2527) against 2567, one zone. A write's whole time barely moved either, 251 -> 240 us: what it no longer waited for it spent elsewhere.
**Why it's dead:** The lock wait wasn't what limited throughput. It also goes against `dax_iomap_rw`, which asserts the lock held for writing.
**Reopen if:** A profile shows writers, not the guest's CPUs, limit a workload.
