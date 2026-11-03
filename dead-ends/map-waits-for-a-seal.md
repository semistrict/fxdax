# A writable MAP short of memory waiting in the daemon for a seal

**Tried:** Analysed and rejected, not built. 2026-11-01, TASK-150.20.
**Idea:** A write short of memory should wait rather than fail, so the daemon could hold a writable
MAP until a seal moves open pages into the cache, where relief evicts them. No guest change, and no
ENOMEM for it to handle. A longer retry in the guest is the same idea from the other end.
**Result:** r460 on the tree before the fix: 262138 of 262144 pages open, relief freeing nothing, and
no commit for 9.6 s, so nothing was coming that a wait could end.
**Why it's dead:** A writable MAP is in the device's gate, and a seal waits for the gate to empty
(`fxfs_device_seal_begin`): the MAP would wait for the seal, and the seal for the MAP. The daemon
can't end that by sealing itself: only the guest can stop its stores to the window. So a MAP has to
fail, leave the gate, and let the guest seal. Nor can the guest seal from `iomap_begin`: a fault
holds its DAX entry there, which a write in the gate may wait for, the same cycle a step out.
**Reopen if:** The daemon can take a window's writable pages away from a guest by itself: evicting
an open extent's pages to the store and faulting them back, or asking the guest to seal.

**Reopened, in part (2026-11-03, TASK-153.15):** the daemon now asks other guests to seal (doc-4's
Sealing), and a writable MAP refused waits for their answers, 10 s at most, before it is tried once
more. Still never for its own guest's seal, for the reason above: its own MAP fails `ENOMEM` and
its guest seals, as before; and a guest asked to seal never waits for another's, which might wait
for it in turn (`crates/fxdax/src/extentd/seals.rs`).
