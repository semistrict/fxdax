---
id: TASK-324
title: >-
  VM process receiving thread munmaps a large reply's payload each message: a
  TLB shootdown per reply (18% of the thread under fxfs-guest's cold reads)
status: To Do
assignee: []
created_date: '2026-11-10 11:38'
labels:
  - perf
dependencies: []
type: enhancement
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-276.6/276.7's bare-metal profile (ubu1, 4 vCPUs, 8 jobs, fxfs-guest, main + the bench): the VM process's 'extentd client' thread spent 18% of its samples in __munmap (unmap_region -> flush_tlb_mm_range -> smp_call_function_many_cond), more than wp_page_copy's 12.5%. wire::Receiver::receive allocates every message's payload with vec![0; len] and the reply is dropped after its request's task copies it out, on the same thread: past musl's mmap threshold each payload is an mmap, zeroed, then a munmap, a TLB shootdown of every CPU running the process's threads (its vCPUs). Callers below __munmap don't unwind (musl's free has no frame pointers), so which messages are large is to confirm (bpftrace on munmap's length).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Which messages' payloads are munmapped measured
- [ ] #2 A receive path that reuses payload buffers past the threshold, measured on the TASK-276 bench (fxfs-guest cold reads) before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
