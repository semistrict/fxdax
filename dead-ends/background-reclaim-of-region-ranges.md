# Reclaiming ranges that hold regions, in the background, as region space runs low

**Tried:** 2026-10-22/23, TASK-91.4, before MAP_MANY (2db1d11c6). Reverted.
**Idea:** During pgbench -i -s 50 with DAX, 33,496 region allocations found no room and 32,399
pieces were mapped in ranges' own space. Keeping regions free would save those MAPs.
**Result:** The load improved: 23 s -> 19.6 s, ~40,300 -> 10,923 MAPs, 405 in ranges. The runs
collapsed: 897 / 563 / 55 tps, regions up to 20,000, unmaps ~58k a run.
**Why it's dead:** Under fsync-heavy load every seal leaves a region, and file ranges slice and keep
them. Regions save MAPs when an extent is written in bulk, not when it holds a few pages of an
fsync's epoch.
**Reopen if:** Regions are used only by bulk writers, or small sealed staging extents stop being
kept as regions. TASK-91.4 raises both as a design question.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): writes no longer MAP the window,
so the regions they filled and the MAPs this saved are gone; the window maps sealed extents
read-only, or zeros (doc-13). Moot as measured.
