---
id: TASK-25.7
title: 'libkrun virtio-fs: per-request latency and mmap reads'
status: To Do
assignee: []
created_date: '2026-10-10 19:08'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 32000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Over libkrun's virtio-fs, with fxdax or passthrough, creating, removing and listing files is 20-60x slower than ext4 and mmap reads ~100x (docs/measurements/gce-fs-bench-2026-10-10T1823Z.md): a request costs 0.1-0.7 ms on GCE under nested KVM, in the guest's notification, the fs worker's wakeup and the interrupt back. And a file's cached pages may be dropped once the guest writes it (AUTO_INVAL_DATA seeing the changed mtime), so mmap reads after writes go to the device. Find where a request's time goes, and cut it: e.g. polling the queue before sleeping, or the writeback cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where a virtio-fs request's time goes on GCE is measured and recorded
- [ ] #2 Creates, unlinks and mmap reads over fxdax faster than in docs/measurements/gce-fs-bench-2026-10-10T1823Z.md, measured on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
