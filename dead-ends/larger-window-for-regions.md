# A larger DAX window so regions stop running out

**Tried:** 2026-10-23, TASK-91.10, 8 GiB window on GCE r63 (experiment, not committed).
**Idea:** The regions' pool was full 75-99k times a run, sending writes to ranges' own space with a
MAP each. Four times the space should keep regions saving those MAPs.
**Result:** Load ~200 seals; run 1 made ~30,000 regions (869 -> 28,568; regions' pool 4 GiB -> 351
MB free). From run 2 the pool was full and writes mapped pages into ranges, 44-67k a run. tps
1018/802/ 750/759/725.
**Why it's dead:** One run fills the larger pool: 38,960 seals leave regions pinned by pages written
once (accounts rows, history), which pruning can't free. A larger window delays the fall-back, no
more. Moving cold pages out of small sealed regions costs the same MAPs.
**Reopen if:** Something other than window size stops pages written once from pinning regions.

**Since:** 2026-11-04, writable DAX deleted (37c331a6c, TASK-164): writes no longer fill regions
with pages written once; the window maps sealed extents read-only, or zeros (doc-13). Moot as
measured.
