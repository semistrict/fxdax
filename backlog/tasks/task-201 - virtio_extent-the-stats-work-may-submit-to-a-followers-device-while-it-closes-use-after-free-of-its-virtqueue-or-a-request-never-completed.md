---
id: TASK-201
title: >-
  virtio_extent: the stats work may submit to a follower's device while it
  closes: use after free of its virtqueue, or a request never completed
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:51'
updated_date: '2026-11-06 09:41'
labels:
  - guest
  - virtio_extent
dependencies: []
references:
  - guest/linux/drivers/virtio_extent/driver.c
priority: medium
type: bug
ordinal: 647000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-11-06 review of the guest's Fxfs (review-guest).

`virtio_extent_close` (guest/linux/drivers/virtio_extent/driver.c:1166), which a follower's last unmount calls through `fxfs_vfs_filesystem_close`, runs `virtio_device_reset_prepare` first: `vext_reset_prepare` resets the device, completes the queued requests with -EIO, and frees the vqs with `del_vqs`. Only after all that does it set `closed`. `virtio_extent_submit` checks `closed` once, without a lock, and then calls `virtqueue_add_sgs(queue->vq, ...)`.

The F_STATS work (`vext_stats_work`, TASK-111.4) can be in `virtio_extent_call` at any time, because the host's asks for stats arrive as configuration changes at any time. Clearing the stats callback (`virtio_extent_set_stats(NULL)`) only waits for the `stats()` call itself, not for the request the work sends after it. So:
- If the work submits after `del_vqs` but before `closed` is set, it adds to a freed virtqueue: use after free.
- If it submits between the detach loop and `del_vqs`, its request sits on a reset device that never completes it. The kworker then waits in `wait_for_completion` for ever, and `vext_remove`'s `cancel_work_sync` hangs with it at unbind or shutdown.

The window is microseconds wide, and it opens every time a follower unmounts or remounts after being cut off (ESTALE recovery) while `fxdaxctl stats` or GetSnapshot asks the VM for its stats. The filesystem's own requests aren't affected, because the filesystem has finished with the device by then. The work is the only submitter left.

Suggested fix (small): in `virtio_extent_close`, set `closed` first, then `flush_work(&vext->stats_work)` (an in-flight request then completes on the live device, and later runs see `closed` and return -ESHUTDOWN), then reset. Alternatively, make submit's check and the reset exclude each other.

Not reproducible on demand: it needs a VM with a follower, the host asking for stats in a loop, and the follower remounting in a loop, run under KASAN (the sanitizer kernels, TASK-44.8) until it trips. A deterministic check would need a delay knob between submit's `closed` check and `virtqueue_add_sgs`, set by the test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 No request reaches a virtqueue that virtio_extent_close is resetting or has freed
- [x] #2 A stats ask racing a follower's unmount ends, answered or -ESHUTDOWN, and unbinding the device afterwards returns
- [ ] #3 A test widening the window (a delay knob, or a KASAN soak of stats asks against follower remounts) that fails on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Per-queue closed flag under the queue lock, read where a request is added; vext_close_queues before a reset; reset_prepare: close queues, flush stats work, reset.
2. Queue locks initialized once (not on reopen).
3. No on-demand test possible (the window is now one critical section): prove by structure; run follower and stats cases on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on guest-199, commit cf362f61d.

Fix: each request queue has a closed flag its lock guards; virtio_extent_submit reads it under the lock it adds the request under (one critical section with virtqueue_add_sgs), and waiters for room wake on it. vext_reset_prepare (virtio_extent_close, vext_remove, and the open path's NEEDS_RESET) closes every queue under its lock, flushes the stats work (a request it already added completes on the live device; any later is refused -ESHUTDOWN), then resets, detaches and frees the queues. Reopening sets each queue's vq and closed=false under its lock; queue locks and wait queues are initialized once, and the number of queues is fixed at probe (a reopen re-initialized the locks a closed queue's submitters may hold, and could have grown nr_queues past the array).

Test: none can make the old race fail on demand, as allowed by the brief. The window was between submit's lockless load of closed and virtqueue_add_sgs; those are now one critical section under the queue lock, which closing takes, so no delay can be put inside it, and a knob before the lock would pass on the old code too (the stats work would then find closed already set). The order is proven by construction: a request is added only under a lock whose holder saw the queue open, and a reset begins only after every queue was closed under that lock and the stats work flushed, so the detach loop sees every request added and nothing is added after. Runs touching the paths: GCE r762 fxfs-guest-follow-remount (followers' close and reopen), fxfs-guest-stats (the stats work), and fxfs-guest-checks' unbind_waits_for_unmount (vext_remove) pass. AC#3 left unchecked for that reason; a KASAN soak of stats asks against follower remounts (TASK-44.8's sanitizer kernels) is the remaining way to stress it, not run.

Blind spot: the stats work (TASK-111.4) added a second submitter outside the filesystem's lifetime, which close's ordering assumed was the only one; no test asks for stats while a follower closes. Look around: no other submitter outside the filesystem exists (selftest runs only via sysfs while bound).

Verified: check-guest.sh arm64 ok; GCE r762 as above.

Decision moved here from OPEN_QUESTIONS.md (2026-11-06):
## The driver's queues close under their locks before a reset, and the stats work is flushed between (TASK-201, 2026-11-06)

`virtio_extent_submit` checked `closed` once, unlocked, before adding to a queue that a reset might free meanwhile. Decided: each queue has a `closed` its lock guards, read under the lock a request is added under; a reset (`vext_reset_prepare`, for `virtio_extent_close` and unbinding) closes every queue, flushes the stats work, then resets, so no request reaches a queue being reset or freed, and the stats work's request in flight is answered by the live device. Queues keep their locks across a reopen (initialized once, their number fixed at probe). No test makes the old race fail on demand: the window was between a lockless load and `virtqueue_add_sgs`, which now share one critical section, so no delay can be put inside it; a knob before the lock would pass on the old code too. *Instead:* the task's ordering alone (closed set, flush, reset), which leaves the unlocked check relying on the flush for its ordering. *Undoing it:* the per-queue flag and `vext_close_queues`.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Queues close under their locks before a reset, the stats work flushed between: no request reaches a queue being reset or freed. Proven by construction (no on-demand test possible, explained in notes; AC#3 unchecked); follower, stats and unbind cases pass on GCE r762.
<!-- SECTION:FINAL_SUMMARY:END -->
