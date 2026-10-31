# Wrong theory: evict_least_used's cost at the memory cap makes each VM slower than the last

**Tried:** 2026-10-25, TASK-108.1, r102 (417c88b12), r110, r111.
**Idea:** At the cap each relief pass gathers and sorts up to 65,536 cached extents under the
cache's lock to free 4 MiB, in order of last cache lookup. That would be slower with each VM's
extents, and could evict base extents every fork maps, fetching them again.
**Result:** r102, 10 VMs: no cliff, a steady decay. Init 19.6 -> 26.2 s, first-run tps 936 -> 827.
Page-ins flat, ~130 MB per VM; every request kind's mean latency up ~20%. r111: extentd's CPU fell,
9.56G -> 8.65G task-clock in 20 s, and its threads parked more.
**Why it's dead:** Re-fetching evicted pages isn't what grows, and extentd is waiting on the VMs,
not busy. The sandbox team's 2.6-3.8x slowdown did not reproduce here. The cause was physical memory
fragmenting (r126).
**Reopen if:** A host shows page-ins growing per VM, or relief passes on a profile.
