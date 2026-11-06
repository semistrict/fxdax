---
id: TASK-201
title: >-
  virtio_extent: the stats work may submit to a follower's device while it
  closes: use after free of its virtqueue, or a request never completed
status: To Do
assignee: []
created_date: '2026-11-06 06:51'
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
- [ ] #1 No request reaches a virtqueue that virtio_extent_close is resetting or has freed
- [ ] #2 A stats ask racing a follower's unmount ends, answered or -ESHUTDOWN, and unbinding the device afterwards returns
- [ ] #3 A test widening the window (a delay knob, or a KASAN soak of stats asks against follower remounts) that fails on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
