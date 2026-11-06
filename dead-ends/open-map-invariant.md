# Host invariant: no read-only MAP of an open extent while another range maps it writably

**Tried:** 2026-10-29, TASK-119.11, 5fc790ffd (FXDAX_CHECK_OPEN_MAPS, not on main), r252.
**Idea:** `map_open` is where an evacuation of an unsealed region would show, so extentd could check
each read-only MAP of an open extent against the window's writable mappings of it and fail the run.
**Result:** 3720 found across checks-dax, -delay, -small, crash-dax and pgbench-dax, all passing.
Nearly all were a small file's slice mapping a page of the open staging extent read-only while its
region mapped it writably; some were map_extents' fallback in a range's own space.
**Why it's dead:** The design doesn't keep this invariant. The host maps one open extent's memory
for every mapping of it, so a read through either sees every store: no disagreement, no data lost.
**Reopen if:** There is a narrower invariant. Telling an evacuation's maps from these needs the
guest's side, as gather sees only sealed regions (`region->evacuate`).

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): the window maps no open extent:
a MAP of one fails EPERM (doc-13, invariant 1), so no writable mapping exists to check against.
