---
id: TASK-276.6
title: >-
  DAX random writes: ~10k PRIVATE a second per VM, each a TLB shootdown on the
  VM process's receiving thread
status: To Do
assignee: []
created_date: '2026-11-09 05:12'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 708000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 phase 1 (GCE c3-standard-8, nested KVM): fxfs-guest-dax random 4K writes stay at 10-12k IOPS for 2-8 jobs and 2-8 vCPUs, latency linear in jobs. Each write is a PRIVATE; the VM process applies every stage operation on its one receiving thread, in order, and that thread is saturated: 47% in mmap of private memory over an installed page and 13% in fallocate, both waiting on TLB-shootdown IPIs (smp_call_function_many_cond, kvm_flush_remote_tlbs), which nested virtualization makes dear. mmap takes mmap_lock for write, so more threads would not overlap them: the cost is per change, and the way out is fewer changes (a guest privatizing more than a page per write where it will write the rest, joining the PRIVATEs a batch of receives holds into one change, or pages written in place that were never installed).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measured design that cuts the stage changes (or their shootdowns) per DAX write, keeping doc-13 rules and the uffd Shuttle tests
- [ ] #2 fxfs-guest-dax random writes scale past 12k IOPS with jobs on the TASK-276 bench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
