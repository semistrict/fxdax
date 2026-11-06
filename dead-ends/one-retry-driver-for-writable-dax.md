# One retry driver for every way into a DAX file, over writable DAX

**Tried:** 2026-11-04, TASK-160.1; tag `archive/task-160.1-retry-driver` (written, never compiled).
**Idea:** -EXDEV and its kin were handled about 24 ways across read, write, fault and zeroing in
dax.c, and TASK-150.13, 150.20, 153.1 and generic/638 were each one path missing what another did.
A table of errors and outcomes per way in, and one driver, would make the paths agree by
construction.
**Result:** the driver and its table were written, with four mutants and two scenarios; read and
zeroing would need drop limits they don't have for its acceptance criteria to hold.
**Why it's dead:** the same day the user decided DAX becomes read-only (the 2026-11-04
first-principles review, docs/reviews/2026-11-04-first-principles.md): DAX maps sealed extents
only, and writes go through the guest page cache. That removes the write and store paths whose
retries the driver mostly governs, and the lock cycle (fault, semaphore, gate, seal) that made
the retries necessary.
**Reopen if:** read-only DAX is abandoned and writable DAX kept.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): copy-on-write DAX (doc-13) is
the only DAX; writable-dax.md.
