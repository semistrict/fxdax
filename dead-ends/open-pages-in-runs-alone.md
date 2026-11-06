# Allocating open extents' pages in runs, on the old free list

**Tried:** 2026-10-29, TASK-131.1, tag archive/wip-open-pages-runs (e3a56c44f, later 585a82004), r263 and
r264.
**Idea:** Open extents' pages were allocated a page at a time, interleaving with every other extent
allocating at once, so their physical runs came out short and each became a stage map.
**Result:** Runs stayed as short: 5.75M stage maps for 54.7 GB (9.5 KB) against 5.69M for 51.9 GB at
a27e50fd8. At 585a82004, 4.23 stage maps a segment against 4.26. Counters then showed commits
already asking for runs: 15.6 pages an allocation, delivered in runs of 2.6.
**Why it's dead:** Asking for runs does nothing when free memory is scattered: the LIFO free list
hands out the short runs freed last. It was dropped alone, and later landed with the allocator it
needed: buddy allocator e88f2c02a plus 9706c857f, r264-G, 27.7 KB per stage map, mean tps 915
against 769.
**Reopen if:** It has landed. It helps only with an allocator that keeps free runs whole.
