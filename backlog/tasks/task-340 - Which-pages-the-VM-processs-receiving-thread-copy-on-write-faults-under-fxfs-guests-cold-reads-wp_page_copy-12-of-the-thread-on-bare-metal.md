---
id: TASK-340
title: >-
  Which pages the VM process's receiving thread copy-on-write faults under
  fxfs-guest's cold reads (wp_page_copy, 12% of the thread on bare metal)
status: To Do
assignee: []
created_date: '2026-11-11 01:11'
labels:
  - perf
dependencies: []
type: enhancement
ordinal: 736000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-276.7 (ubu1, bare metal, 4 vCPUs, 8 jobs, fxfs-guest's scaling group, perf of the VM process): the 'extentd client' thread spends 12.5% of its samples in wp_page_copy (do_wp_page, then ptep_clear_flush's TLB shootdown), under memcpy and recvmsg's copies. TASK-276's GCE profile had taken them for guest pages the guest read before the host wrote (the zero page), but a guest booted with init_on_alloc=1, which writes every page it allocates, left them at 11.7% (and an interleaved A/B of the two, 3 rounds x 2, found every cold-read IOPS and latency within drift). So which mappings they are is open: guest memory a read mapped read-only otherwise, the transfer area, or the receiver's heap.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The mappings whose pages the thread COW-faults named, with how they came to be read-only
- [ ] #2 Whether keeping them writable pays, measured on the TASK-276 bench
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
