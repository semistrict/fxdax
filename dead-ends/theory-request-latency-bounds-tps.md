# Wrong theory: cutting device request latency raises pgbench tps

**Tried:** 2026-10-20/21, TASK-41: EVENT_IDX and FREE_MANY (49dbcd3), kept frees indexed (49d1c1c),
segments framed off the commit, thread hand-offs cut (93a73fc).
**Idea:** The whole host was 78% idle under pgbench, so the time had to be latency, and extentd and
transport had the visible cost.
**Result:** Requests got faster: store phase 106 -> 11 us, extentd WRITE 180 -> 108 us, guest-side
WRITE 316 -> 278 us, MAP 199 -> 177 us. pgbench-init fell 4.07 -> 3.25 s (1.24x -> 1.01x ext4).
pgbench-tps stayed flat at ~1,366 through every cut.
**Why it's dead:** pgbench runs 4 clients on 2 guest vCPUs, and tps was bound by the guest's CPU,
not the requests' latency. The cuts were kept for the load; they just aren't a route to tps.
**Reopen if:** The guest stops being CPU-bound during the run.
