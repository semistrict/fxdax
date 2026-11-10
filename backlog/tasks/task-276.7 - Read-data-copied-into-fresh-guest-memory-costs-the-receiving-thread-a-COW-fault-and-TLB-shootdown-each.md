---
id: TASK-276.7
title: >-
  Read data copied into fresh guest memory costs the receiving thread a COW
  fault and TLB shootdown each
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 05:12'
updated_date: '2026-11-10 12:30'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 709000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 phase 1 profile (fxfs-guest, cold random reads, 4 vCPUs): a third of the VM process receiving thread is in wp_page_copy -> ptep_clear_flush -> smp_call_function_many_cond as it copies READ replies into guest pages the host never wrote (the zero page, mapped by an earlier guest read), each first write a copy-on-write fault and a TLB shootdown to every vCPU. Per-read latency is not bound by it yet (flat 150-190 us at 1-8 jobs), but it is serial work on the thread every reply passes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Measured whether populating guest memory writable (or completing reads off the receiving thread) removes the shootdowns from the reply path, and done if it pays
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 1, bare metal (ubu1, fxjob --bench, main + bench, 4 vCPUs, 8 jobs, perf of the VM process): fxfs-guest cold random reads 8.2k / 13.4k / 19.8k / 25.3k IOPS (j1/j2/j4/j8; ext4 on virtio-blk 23.0k / 53.9k / 52.5k / 48.5k), p50 42 / 49 / 64 / 94 us. The receiving thread ('extentd client') is busy 54% of a CPU at most in any second; of its samples over the case: wp_page_copy 12.5% (the COW of guest pages its read replies are copied into, each a shootdown), munmap 17.96% (each large message's payload, vec![0; len] past musl's mmap threshold, unmapped as the reply drops: TASK-324), memcpy 21%, recvmsg 21%, waiting 12%. Not bound by the thread on bare metal.
<!-- SECTION:NOTES:END -->
