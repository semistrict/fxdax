---
id: TASK-92.13
title: >-
  FREE_MANY runs on the queue's taker thread, one blocking daemon call per
  extent
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 288000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
quick() counts every FREE as quick without checking FREE_MANY (worker.rs:129-144), so a FREE_MANY popped alone is served on the taker (302-305). It frees each extent with its own call_blocking (547-560), and meanwhile the queue takes in nothing. The frame run served about 50k FREEs at 205-218 µs each, about 2.4 extents each, which blocked queue intake for about 10 s in total, and the next flush waits on them (journal.rs:1833). A 257-extent batch takes about 8.7 ms serially.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:129-144, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:302-305, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:547-560, crates/fxdax/src/extentd/client.rs:339-342, crates/fxfs/src/object_store/journal.rs:1833.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Partly fixed. Firecracker 0a6f1e332 (fxdax 93a73fc, TASK-41) removed quick() and the taker's inline serving. Each request is now a future that runs on the worker only until its first await, so a FREE_MANY no longer blocks queue intake (vendor/firecracker/.../extent/worker.rs:100-128). Still there: FREE_MANY awaits free_async once per extent in sequence (worker.rs:522-536), each a separate FREE round trip (crates/fxdax/src/extentd/client.rs:395-398). There is no concurrent join and no daemon-side FREE_MANY, so a 257-extent batch still takes 257 serial round trips. Related: TASK-41, TASK-79.22.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
