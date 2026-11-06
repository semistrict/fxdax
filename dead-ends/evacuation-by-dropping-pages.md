# Evacuating by dropping pages instead of mapping them

**Tried:** 2026-10-28, TASK-109.1 attempt 2 (r201, r202, r203). Reverted.
**Idea:** Pages would leave the staging region for their range's own space unmapped, to be mapped
there when next used. r112 had shown evacuated pages are mostly cold, so most of those MAPs would
never be made.
**Result:** mmap_write, zero_mapped, reflinks and random_ops read zeros under delay injection.
**Why it's dead:** A range's `mapped` bit means the file has data there. A page not set in it reads
as a hole (`fxfs_dax_iomap_begin`'s read path), so a dropped page's data disappears. TASK-109.1 had
already noted this before the attempt.
**Reopen if:** Ranges record "the file has data here" apart from "the window maps this page", or a
dropped page makes its whole range be forgotten (see drop-idle-ranges-at-evacuation.md for the cost
of that at a range's grain).
**Since:** 2026-10-31 that lack was made up, and it still lost: evacuation-leaving-pages-absent.md.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation is gone. dax.c now
maps a page before use rather than read an unmapped one as a hole (guest/sim's
unmapped-page-read-as-hole mutant). Moot.
