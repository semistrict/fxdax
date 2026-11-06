# DAX write(2) holding the invalidate_lock across dax_iomap_rw

**Tried:** 2026-10-24, TASK-79.12 and TASK-79.28 (one change, reverted).
**Idea:** Held exclusively across the copy, the lock means no fault holds a DAX entry the write
waits for in the gate, which breaks TASK-79.12's deadlock cycle. It also let write_iter fit the
ranges under one hold, so a fault couldn't recreate a range from the old i_size (TASK-79.28).
**Result:** fxfs-guest-crash-dax hung at its first step on GCE, no output in 15 s. Bisected to the
write_iter change: without it crash-dax passes. The hang was not diagnosed further.
**Why it's dead:** Holding the lock across the copy hangs the guest, for a reason nobody found. Both
bugs were fixed otherwise: faults take file->sem only by trylock and retry without their DAX entry
(718788ca9); a range too small for the write is dropped and mapped again (97e9fa8ee).
**Reopen if:** The hang is diagnosed and something needs this lock held across the copy.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): the gate and moves are gone, and
with them both bugs' causes; copy-on-write DAX's one locking rule is the file's semaphore (doc-13,
invariant 3). Nothing needs this lock across the copy for them.
