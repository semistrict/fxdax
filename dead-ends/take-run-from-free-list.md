# Starting a multi-page allocation where the first free pages on the list begin a run

**Tried:** 2026-10-26, TASK-108.1, f7893385b (Arenas::take_run), r139. Reverted.
**Idea:** Runs of physical pages shorten as memory recycles, and each run is a stage map. 9c94b62e1
extended a run only where the page after the last was free, so picking a good start among the first
32 on the free list should keep runs long.
**Result:** No change over 10 VMs: pages per stage map 10.3, 7.9, 6.6, 5.8, 5.5, 5.3, 5.1, 5.1, 5.1,
5.1 (r134: 10.1 -> 5.1); first-run tps 1110 -> 1049-1057; VMM system 91.8 -> 106.0 s.
**Why it's dead:** The runs that shorten aren't multi-page allocations' starts. TASK-131.1 later
showed free memory itself scattered: the free list, LIFO, hands out the short runs freed last.
**Reopen if:** Nothing in this form. What worked was keeping free runs whole: fxdax's physical
memory as a buddy allocator (e88f2c02a, TASK-131.1), 9.5 -> 27.7 KB per stage map.
