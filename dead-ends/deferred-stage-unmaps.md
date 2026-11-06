# Deferring and sending STAGE_UNMAPs outside the MMU lock (the r7 DAX branch)

**Tried:** 2026-10-23/24, TASK-91.16 and TASK-92.50: 7 commits on review/r7-dax-unverified, on main
da7726d8, among them 91fc372f and 491d3838. Handed over as not to be landed.
**Idea:** MAP's cost fit a waited STAGE_UNMAP per overwrite of a populated window page, made inside
map_unsafe under the aspace, VMO and MMU locks. Deferring it, and reprotecting a seal's pages in
place, would cut the VMM's host time.
**Result:** 'extentd client' system time per MAP 189 -> 158 us on GCE. The branch's review found
three ordering races: a deferred STAGE_UNMAP sent after another thread's fault mapped the same
offset; STAGE_PROTECT not undoing a restriction the fault thread made with mprotect; acknowledgement
IDs out of order on the wire once waited unmaps left the MMU lock. It also clashed by design with
review/r2b-extentd, whose RemoteStage never waits.
**Why it's dead:** Deferring opens those races. What paid was redone without it: corked window
changes with in-place reprotection in the VM process (pushed at e3d46ea92, ~112 -> ~85 us per window
change) and the stage counters (ae046fc9a).
**Reopen if:** Protection has to be deferred and batched to save flushes. The Shuttle model
faults_race_deferred_unmaps on the branch covers the races.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): seals no longer write-protect
the window and STAGE_PROTECT is gone, so there is no protection to defer or batch.
