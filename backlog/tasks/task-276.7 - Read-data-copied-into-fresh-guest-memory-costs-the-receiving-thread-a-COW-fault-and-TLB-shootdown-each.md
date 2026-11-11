---
id: TASK-276.7
title: >-
  Read data copied into fresh guest memory costs the receiving thread a COW
  fault and TLB shootdown each
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 05:12'
updated_date: '2026-11-11 07:25'
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
- [x] #1 Measured whether populating guest memory writable (or completing reads off the receiving thread) removes the shootdowns from the reply path, and done if it pays
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 1, bare metal (ubu1, fxjob --bench, main + bench, 4 vCPUs, 8 jobs, perf of the VM process): fxfs-guest cold random reads 8.2k / 13.4k / 19.8k / 25.3k IOPS (j1/j2/j4/j8; ext4 on virtio-blk 23.0k / 53.9k / 52.5k / 48.5k), p50 42 / 49 / 64 / 94 us. The receiving thread ('extentd client') is busy 54% of a CPU at most in any second; of its samples over the case: wp_page_copy 12.5% (the COW of guest pages its read replies are copied into, each a shootdown), munmap 17.96% (each large message's payload, vec![0; len] past musl's mmap threshold, unmapped as the reply drops: TASK-324), memcpy 21%, recvmsg 21%, waiting 12%. Not bound by the thread on bare metal.

Measured (2026-11-10, ubu1 bare metal, fxjob --bench, the branch's build): an interleaved A/B, the guest booted with init_on_alloc=1 (every page it allocates written by it, so that no guest page is the host's zero page when a READ reply is copied in) against the default, fxfs-guest and ext4 the control, 4 vCPUs, 3 rounds x 2 iterations: 0 real differences; cold reads j1/j2/j4/j8 7.25k/11.7k/18.0k/19.9k against 6.94k/12.0k/18.8k/19.9k IOPS, qd16 48.2k/46.6k, p50s within drift. And a profile of it (8 jobs) left the receiving thread's wp_page_copy at 11.7% (12.5% without): the COW faults are not of pages fresh to the guest, so populating guest memory writable would not remove them either, and would make every VM's RAM resident at boot (2 GiB a bench VM; doc-13's density, hundreds of forks a host) for nothing shown. Completing replies off the receiving thread moves the faults to the queues' workers, a wake-up each request that TASK-41 removed, and the thread is not bound on bare metal (54% busy at most in any second). Decision: neither done. What the faulted pages are is TASK-340; the larger shootdown on the reply path, munmap of large payloads (18%), is TASK-324.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Measured, not done: on bare metal (ubu1) the receiving thread is not bound under fxfs-guest's cold reads (54% busy at most), and a guest booted with init_on_alloc=1 neither removed its copy-on-write faults (wp_page_copy 12.5% -> 11.7%) nor moved any read's IOPS or latency past drift (interleaved A/B, 3x2): the faulted pages aren't fresh to the guest, so populating guest memory writable, which would make every VM's RAM resident at boot, would not help; completing replies off the thread would only move the faults and add a wake-up a request. Which pages they are: TASK-340; the larger reply-path shootdown, munmap of large payloads (18%): TASK-324. docs/measurements/dax-lazy-install-2026-11-10.md.
<!-- SECTION:FINAL_SUMMARY:END -->
