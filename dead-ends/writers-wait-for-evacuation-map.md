# Writes waiting, holding their file's semaphore, for an evacuation's MAP in flight

**Tried:** 2026-10-24, TASK-101, 217622edc, r91.
**Idea:** An evacuation's read-only MAP could land after a write's writable map of the same page in
the range's own space, so the host refused the write's stores and the VM stopped. Having writes wait
for the file's evacuation MAP closes that.
**Result:** Deadlocked in r91 round 1. The writer waited holding its file's semaphore in the gate;
the evacuation, gathering another file, waited for that file's semaphore, held by a writer waiting
for the gate, which a seal kept closed for the first writer. The mutant without the wait passed 8
rounds of log_writes, so the race wasn't shown either.
**Why it's dead:** The wait sits inside the gate with a semaphore held, and one evacuation crosses
files. TASK-101 landed the evacuation holding the semaphore for writing from gathering through
placing (5183ab50d) instead.
**Reopen if:** The writer lets go of its semaphore and leaves the gate before it waits, which is
what TASK-112.5's back-off has to do.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation and the gate are
gone. Moot.
