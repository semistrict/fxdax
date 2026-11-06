# Wrong theory: TASK-133's zero page came from a MAP opening an extent racing a commit

**Tried:** 2026-10-31, TASK-133, in reasoning about the zero pages; not written in the task's notes.
**Idea:** Torture's page became a zero page mid-phase, and TASK-121 had already shown a host race on
extent opening putting zeros over data. A MAP opening an extent (`ExtentStore::map_open`) racing a
commit looked like the same family.
**Result:** Dropped.
**Why it's dead:** It can't happen. `commit_and_seal` holds the `open` lock throughout, and
`map_open` takes it, so a MAP opening an extent comes wholly before or after a commit. The race
suite (TASK-119.19, read-only map x commit, writable map x commit) and specs/extent-open
(TASK-119.20) agree. The cause was in the guest: a fault leaving with its blocks moved and unmapped
(e4b6b24d7).
**Reopen if:** `commit_and_seal` stops holding `open` throughout.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): `map_open`, `commit_and_seal`
and specs/extent-open are gone; the window never maps an open extent (doc-13, invariant 1), so the
race can't be posed.
