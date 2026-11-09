---
id: TASK-276.4
title: >-
  The device's daemon path: no serialization across queues where the profile
  shows one
status: Done
assignee: []
created_date: '2026-11-09 03:54'
updated_date: '2026-11-09 09:18'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
All queue workers of an extent device share one DaemonConnection: one client receiving thread completes every queues requests, the daemon serves at most WORKERS (8) of a connections requests at once, and the transfer area has 8 slots. Whether any of these caps scaling is for the phase-1 profile to say.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The phase-1 profile says whether the daemon path serializes, and where
- [x] #2 If it does, the change removes it, keeping extentds per-store invariants (doc-4, doc-13), with Shuttle/concurrency tests; if not, the task records why it was skipped
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Profile (phase 1): the connection doesn't serialize: the client keeps up to 4096 requests in flight, extentd serves 8 a connection at once, neither saturates. The one serial step is the VM process's receiving thread applying stage operations in order, saturated by DAX writes' PRIVATEs (mmap 47%, fallocate 13%, mostly TLB-shootdown IPIs, under mmap_lock for write: more threads would not overlap them; TASK-276.6). Tried: KVM's prefault of private runs (7.6% of the thread) moved to a thread of its own (5ab0927d6). Interleaved A/B (fxfs-guest-dax + ext4 control, 4 vCPUs, 3 rounds x 2 iterations): no IOPS change beyond drift, inline 1.08x ahead at 1-2 jobs (within drift), and rand-write j8 p99 1,073 -> 1,155 us (real): the guest's store now reaches the page before the prefault and pays the nested fault itself. Dropped, not landed. No daemon-path change lands from this task; the ceiling is TASK-276.6's.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The connection doesn't serialize (client 4096 in flight, extentd 8 a connection at once). The one serial step is the VM process's receiving thread applying window changes in order, bound by TLB-shootdown IPIs under mmap_lock, which more threads can't overlap: TASK-276.6. Moving KVM's prefault off that thread was measured (A/B) no better, its j8 p99 worse, and dropped. No change landed.
<!-- SECTION:FINAL_SUMMARY:END -->
