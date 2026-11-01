# Theory: extentd lets a VM process go because a stage operation hung, or host I/O stalled it

**Tried:** 2026-11-01, TASK-148, on main at 29a58fa3d, in Lima on a Mac, so nested virtualization.
**Idea:** "left a stage operation unacknowledged for 5s", with the client thread seen in `mmap` under
`leave_out`, read as one operation stuck, perhaps on a full disk or a cold page cache.
**Result:** `enospc` alone: 1 of 23 runs, with 0 of 8 under cold caches, 0 of 8 under disk load, and
0 of 8 with the seed the failures had. Timing every operation over six full runs, none took over
240 ms. The kernel stack was in state R in `kvm_pgtable_stage2_unmap`, working, not waiting. The run
killed had `acked` climbing 10,900 a second, some 46,000 operations queued.
**Why it's dead:** nothing hung. The watchdog waited for the newest operation it had found, so a burst
taking over 5 s to work through read as silence. Fixed by counting silence, with a cap on a backlog.
**Reopen if:** a VM process is let go with acknowledgements stopped: then something did hang.
