# Wrong theory: crash-dax's lost fsyncs were a durability bug in the store or journal

**Tried:** 2026-10-18/19, TASK-26.6.2, 18 runs without a recurrence, crates/fxdax/examples/store-tree.rs, the
extents-at-crash copy (afd84e4). Hypothesis (d) is also TASK-68.
**Idea:** The store recovered a clean prefix, ~23 fsyncs per writer short. So (a) extentd
acknowledged a durable COMMIT before its index delta was durable; or (b) journal replay stopped
early at a block it rejected; or (d) a barrier write went out with an earlier journal write in
flight, leaving a hole.
**Result:** The cause was the test's host check. Before 8613c65's cross-process lock, it could open
the store while the daemon was still flushing, and recover it as of the last background flush,
0.1-0.2 s of fsyncs short. With the lock disabled, the new handover test fails at round 1.
**Why it's dead:** No store or journal fault was involved. Fxfs's overwrite submits POST_BARRIER
only after the others complete and journal flushes are serial, so (d) was checked by reading, not by
a test. The soak with the lock: 1,002 rounds on 3 GCE hosts, no acknowledged fsync lost.
**Reopen if:** A clean-prefix loss recurs with the lock in. TASK-68 still wants (d) proved.

**Since:** 2026-11-04, TASK-68 closed (ffcae1f63): every barrier or commit begins a Seal that waits
for the device's writes in flight (guest/rust/fxfs-linux/src/device.rs, `Seal::begin_with`), so (d)
can't happen.
