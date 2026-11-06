# Holding the file's semaphore for reading through the evacuation's MAP

**Tried:** 2026-10-24, TASK-101 (9c025a0ee, amended by 5183ab50d); 2026-10-28, TASK-112.4 (a
downgrade from write to read for the MAP, reverted).
**Idea:** Readers would stop waiting for the ~400 us MAP inside each ~650 us hold, with the file
still protected from a write's map landing before the evacuation's.
**Result:** On 2026-10-28: write holds 32.7 -> 15.6 s, read waits 17.8 -> 5.3 s, but writes mapping
anew (the WAL's) waited 7.5 s, up from 1.6 ms, and tps did not move (1163, run 1). It also hung
checks-dax under fxfs.dax_delay_us=200: the placing waited for range users to drop to one, which no
put woke, as the release fires only at zero. In TASK-101 the read hold failed regions_evacuated in
r95 round 1, 137 regions more after 400 fsyncs: a write queued behind the MAP took the range before
the placing, which skipped its runs.
**Why it's dead:** Writers need the semaphore for writing, so they keep waiting behind the MAP, and
the placing then races them for the range.
**Reopen if:** Nothing in this form. The live design is TASK-112.5, a MAP with no semaphore held.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation is gone, TASK-112.5's
MAP with no semaphore held with it. Moot.
