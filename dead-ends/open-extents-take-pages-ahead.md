# Open extents reserving 16 pages ahead of their next run

**Tried:** 2026-10-30, TASK-131.1, b6051f15c, r261 against 8fc401ea9. Not pushed.
**Idea:** An open extent growing commit by commit takes its next pages from wherever memory is free.
Reserving pages physically after the last should let the next commit continue the run.
**Result:** tps 1169.7, 1076.0, 1003.3, 988.0, 1036.4 against 1170.3, 1024.6, 1005.6, 985.0, 1017.3,
within the spread. Stage maps a window MAP 2.84 1.72 2.06 1.99 1.89 against 2.83 1.72 2.01 1.95
1.80. Open extents' physical runs an allocation rose, 1.01..1.21 against 1.01..1.09.
**Why it's dead:** The reservation doesn't shorten what windows map. The pages ahead are cleared
whenever the next commit isn't at the page after, which splits runs rather than joins them. Open
extents' maps are only ~15% of stage maps; the rest are cached extents'. Its unit test passes with
it and fails without (AHEAD_PAGES = 0), so it does what it says.
**Reopen if:** A workload's open extents grow page after page across commits, and open extents' maps
are most of its stage maps.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): the window maps no open extent
(a MAP of one fails EPERM, doc-13), so open extents have no stage maps. Moot.
