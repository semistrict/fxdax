# Retiring a fork's reference by emptying it in place, a tombstone a stale create finds there

**Tried:** 2026-11-14, TASK-395.13 and TASK-395.14, in specs/bucket/Bucket.tla only (rule
`retirementsKept`, mutation `NoRetirementsKept`, which TLC failed as meant). Never in the code.
**Idea:** A reference is create-only. Retired, or taken back, overwrite it with an empty object
and keep it, rather than delete it: a writer whose read of the child's record came before the
retirement then finds its key taken and writes nothing, so a retirement is for good, and dead
cells need not retire their references again every pass.
**Result:** The model held for the writers' side, but the collector's side breaks. A pass over the
parent's cell finds the fork points it keeps by delimiter listings of `<C>refs/<parent>/`: the
prefixes, not the references (a clone's reference and its point's share the fork point's prefix,
the clone's written before the point's is retired, so a prefix listing at any time shows it).
With tombstones in place, every fork point that ever had a reference keeps a key under its
prefix, and its state, for good. Telling tombstones apart needs each fork point's references listed
one by one, every pass, and that per-key listing races the clone and its point as the prefix one
does not.
**Why it's dead:** The tombstone has to be at the reference's own key to stop a create-only write,
and that key is under the prefix the parent's pass keeps fork points by.
**Reopen if:** The parent's pass lists its references one by one anyway (it keeps per-reference
state), or fork points are kept by something other than their prefix holding a key.

Instead (landed): the mark is a key of its own, `<C>retired/...`, beside the reference and outside
`<C>refs/`, written before the reference is deleted and kept; a pass over the parent's cell
deletes the references it lists beside their marks. A stale write changes that cell's listing, so
its next pass goes in full and finds it (rule `retirements_marked`, TLA+ `NoRetirementsMarked`).
