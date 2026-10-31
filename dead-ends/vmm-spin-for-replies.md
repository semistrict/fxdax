# The VMM spinning 50 us for extentd's replies

**Tried:** 2026-10-20, TASK-41, batch 2. Reverted.
**Idea:** The host's side of a WRITE was ~300 us with the host 78% idle, so the time was latency
through ~6 thread hand-offs. Spinning instead of sleeping would avoid a wakeup.
**Result:** It gave nothing.
**Why it's dead:** TASK-41 records only that it gave nothing; the host's side of a WRITE was ~300
us, well past a 50 us spin. Cutting the hand-offs themselves worked for requests (93a73fc:
guest-side WRITE 316 -> 278 us).
**Reopen if:** Replies come within the spin. Guest-side polling is a separate question:
virtio_extent.spin_ns is being measured under TASK-146.1.
