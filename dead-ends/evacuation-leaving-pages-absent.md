# Evacuation leaving pages unmapped, a read mapping them on demand

**Tried:** 2026-10-31, TASK-146.2, branch night/evacuate (fxfs.dax_evacuate_cold), r369, r372, async-commit pgbench-dax.
**Idea:** 83% of the window's maps were evacuation's, clogging the VMM's one thread. evacuation-by-dropping-pages.md said pages were mostly cold and only lacked a bit saying "the file has data here, the window doesn't map it"; with an `absent` bitmap, evacuation needs no MAP.
**Result:** It worked as a mechanism: evacuation's maps 994k -> 0, holds 60 -> 11 s, data checks passing 3 rounds under delay injection. But tps fell: 2314/2329/2323 against 2567, same zone. Under this load the pages aren't cold: 2.0M of 2.8M absent pages were mapped back by 390k fills, own-range maps 128k -> 1.1M, each fill 470 us holding the file's semaphore for writing, on the read path. The first cut (r369, 1909 tps) was worse as fills made regions, whose space writes then lacked: regions_full 117k -> 837k; mapping fills in own space fixed that. Regions still ran out ~110k times, and writes' lock wait stayed ~130 us.
**Why it's dead:** It moves the MAPs from the background into reads, in a workload that reads what it wrote.
**Reopen if:** A workload's evacuated pages really are cold, or the fills needn't hold the semaphore. One unexplained failure stands against it: prune_while_paused read zeros once in r369, the knob back off by then; 6 later rounds with it on didn't repeat it.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): evacuation is gone. Moot.
