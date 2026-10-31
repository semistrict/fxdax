# Durable but still open staging extents, sealing only on fork, snapshot or reclaim

**Tried:** Analysed, not built. Appendable extents rejected in doc-4's design review, 2026-10-13;
the sandbox team's round 7b question answered in doc-9, 2026-10-28 (TASK-112.3).
**Idea:** Fxfs never rewrites a committed block in place, so a commit could make an extent's written
prefix durable and keep the rest writable, saving the restage and the evacuation after each fsync.
**Result:** It would cost a store format whose extents grow across commits, prefix-level protection
of window pages, and seal-on-share in forks, snapshots and followers.
**Why it's dead:** Durability is of whole extents, packed in commit order; sharing by ID and the
shared page cache are safe only because an extent never changes once a commit has named it. The
committed prefix would still need the same write protection before anything else could map it, so
deferring the seal would not remove that cost. Doc-9 ranks this last of its levers.
**Reopen if:** Doc-9's cheaper levers don't close the gap (its section 2, item 5).
