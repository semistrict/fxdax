# Have a shared PMD read fault fall back to PTEs, to keep it from dax_associate_entry's WARN

**Tried:** 2026-11-01, TASK-149.14, guest/sim/fixes/shared-pmd.patch at 0a30c4c53.
**Idea:** `fxfs_dax_iomap_read` said a fault's pages were shared and made the iomap a page short of a
PMD if a slice or a move had them in a region, so that `dax_iomap_pmd_fault` falls back. 3,000 seeds
passed with it.
**Result:** `tests/pmd_over_stale_index` WARNs at fs/dax.c:458 with it. A range's own space says
shared too, through a page relocated (`fxfs_dax_cow`), which its condition missed. Moving the
condition after `fxfs_dax_cow` can't work: `fxfs_dax_cow` forgets pages it says are shared because
the fault replaces their entries, and a fault that falls back replaces none.
**Why it's dead:** it treats a symptom in the one caller the seeds reached. The cause is in fs/dax.c:
`dax_folio_put` leaves an order 0 folio's index, which is its share count, to look like a share.
**Reopen if:** a PMD's fault of pages really in other entries (a reflink's) WARNs with the share count
cleared: then those faults do need to fall back, and to keep `fxfs_dax_cow` out of it.
