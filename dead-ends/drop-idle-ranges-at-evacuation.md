# Evacuation dropping ranges idle for a while, rather than mapping their pages again

**Tried:** 2026-10-26, TASK-91.16, 16d8fa399 (fxfs.dax_drop_idle_ms), reverted by c6d96357e. r136,
one host.
**Idea:** A window change that takes access away costs a TLB flush, so mapping cold pages a second
time is waste. Dropping a range no iomap has used lately leaves it to reclaim, as the window's
reclaim does.
**Result:** 0 ms: 1131/1058/1011/971/981 tps. 1000 ms: 740/128/115/76/54, ~3k ranges dropped a run,
then 124k-324k 'range' MAPs a run as data files' chunks, read again, were mapped anew piece by
piece. 5000 ms: 1131/1039/990/957/948, 77 ranges dropped in all.
**Why it's dead:** The ranges evacuated are in use within 5 s: the WAL's chunk being written, data
files being read. A short limit drops ranges still in use, and a long one drops almost nothing.
**Reopen if:** Dropping works at a page's grain, not a range's. That needs what
evacuation-by-dropping-pages.md lacks.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation is gone. Moot.
