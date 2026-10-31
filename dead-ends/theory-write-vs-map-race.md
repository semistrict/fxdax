# Wrong theory: TASK-133's zero page came from a device WRITE racing a MAP

**Tried:** 2026-10-31, TASK-133, in reasoning about the zero pages; not written in the task's notes.
**Idea:** TASK-121's last loss was exactly this: ExtentStore::write checked an extent wasn't open,
let go of the lock, then wrote the store, and a writable MAP opening the extent between made its
commit write zeros over the write. The symptom matched, so a path around that fix looked likely.
**Result:** Dropped.
**Why it's dead:** It can't happen since cae97000a. ExtentStore::write makes its store write holding
the `open` lock, and `map_open` takes that lock, so the MAP reads the store before the write or
after it. The race suite (TASK-119.19) fails with that fix taken out, and specs/extent-open
(TASK-119.20) holds with it. The cause was in the guest: a fault leaving with its blocks moved and
unmapped (e4b6b24d7).
**Reopen if:** ExtentStore::write stops holding `open` across its store write.
