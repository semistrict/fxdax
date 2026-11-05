# A writer's fence naming the checkpoint it recovered from, so that recoveries ignore later ones

**Tried:** 2026-11-05, TASK-175, designed and rejected before code (branch task-175).
**Idea:** a fenced writer's checkpoint, landing after the next writer recovered the branch, names
extents further along their moves than that writer does, and the collector deletes the moves the
writer still needs. If the fence named the checkpoint its writer started from, recoveries (the
collector's among them) could ignore any checkpoint of an earlier epoch newer than that one, and
the collector's state would be the writer's.
**Result:** a counterexample on paper. Between a writer's recovery and its fence landing, the
writer before is still the branch's by its segments, and a checkpoint of its own landing then is
valid by the rule: the collector trusts it, and deletes what the older checkpoint names. Once the
fence lands, the rule sends every recovery back to the older checkpoint, whose objects are gone.
**Why it's dead:** a checkpoint's validity changes from valid to invalid when the fence lands, and
what the collector deleted while it was valid can't come back. Whatever the collector trusts must
stay trusted: validity has to be monotone in what the bucket holds. The fix kept is the collector
trusting a checkpoint only when the lease, read after it, is its writer's, and otherwise keeping
every moves from extents' seals (doc-7, Garbage collection, step 5).
**Reopen if:** checkpoints become confirmed by later footers (each naming its writer's newest),
which is monotone, and something needs the collector's state to be exactly its writer's.
