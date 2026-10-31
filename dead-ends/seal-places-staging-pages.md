# The host placing a seal's staging pages at their ranges' addresses during the seal

**Tried:** Analysed, not built. TASK-91.10 and TASK-109.1, parked 2026-10-25.
**Idea:** A written page would take no MAP of its own: the device moves a seal's staging pages to
their ranges' addresses as part of the seal's remap, which replaces the evacuation (77,010 of
120,091 MAPs in a 60 s run). It needs a protocol change.
**Result:** It would put about 2 host mappings, ~0.35 ms, on every fsync's critical path, where the
asynchronous evacuation puts none.
**Why it's dead:** It swaps background work for work on the commit path. The per-MAP host cost is
the root, and doc-9 later showed a DAX commit already costs ~0.43 ms against ~0.26 ms without DAX.
**Reopen if:** The cost of a host mapping falls far enough that two of them on the commit path no
longer matter. TASK-91.16 took host time per window change from ~112 to ~85 us, and its 50 us per
MAP target was set aside.
