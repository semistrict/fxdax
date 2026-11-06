# Wrong theory: late evacuations lose data by mapping an extent whose seal hasn't ended

**Tried:** 2026-10-28, TASK-112.6. Injection fxfs.dax_evacuate_in_seal; fix `region->sealed_at` on
branch wip/task-112-5 (9b937d632); diagnostic fxfs.dax_evacuee_verify (54aaf0633).
**Idea:** `fxfs_dax_seal` marks regions evacuable as the seal begins, before the device has sealed
their extents, so an evacuation in between maps a still-open extent for good.
**Result:** Injection on main: checks-dax passed 3 of 3 (r202), not reproduced. With sealed_at, the
same loss: random_ops failed round 2 of 10 (r204). With verify on, r205 failed round 6 with no
'evacuated page differs' warning, so each page matched its region when placed.
**Why it's dead:** The losses were TASK-121's host race, ExtentStore::write against a writable MAP
opening the extent (cae97000a). Under fxfs.dax_evacuate_every=16, r218 with that fix taken out
failed round 1, and r221 with it passed 8 of 8. A review also found the host's `seal_mapped` remaps
every mapping of the open extent, which makes evacuating an unsealed region look harmless there.
**Reopen if:** A trace shows an evacuation's read-only map of an open extent itself losing data.
open-map-invariant.md found such maps are routine and harmless.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation is gone, and the
window maps no open extent (doc-13, invariant 1). Moot.
