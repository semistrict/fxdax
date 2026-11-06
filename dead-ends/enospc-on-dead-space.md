# Bounding a store's host disk by refusing room once dead space passes its capacity

**Tried:** TASK-79.2, replaced by f5800c11 on 2026-10-23; the bound across commits became
TASK-79.89.
**Idea:** A guest that frees as fast as it writes grows the host's files without limit. Counting
dead space against a branch's room refuses such a guest with ENOSPC.
**Result:** It refused an honest DAX guest's journal writes with its store a tenth used: the
uploader releasing its kept frees turned 320 MiB into dead space at once. Cleaning's copies, which
wait for the guest's next commit, could then never land, so the store stayed full for good.
**Why it's dead:** An honest guest can have dead space appear at once, and refusing its writes stops
the commit that lets cleaning take the space back. What landed is a cap on host files at 3x capacity
in disk bytes, with commits, seals and cleaning never refused, a credit of at most one capacity for
a commit's own writes, and the uploader's holdings left out (3ca2e731; the user accepted a 4x
effective ceiling).
**Reopen if:** Nothing in this form.

**Since:** 2026-11-05, local cleaning deleted (a31f82a98, TASK-165.8): the 3x-capacity bound,
cleaning's copies and `Usage::dead_bytes` are gone. Acknowledged segments leave the store for the
host's part cache (`Cache::adopt`), and the bucket's collector is the only cleaner.
