---
id: TASK-109
title: >-
  Sandbox team, round 6 (2026-10-25 report): DAX about 45% slower than no DAX
  under pgbench at b746104
status: To Do
assignee: []
created_date: '2026-10-25 23:26'
labels: []
dependencies: []
priority: high
ordinal: 409000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team profile at b746104 (GCE n2-standard-16, us-central1-a, nested, RustFS 25 ms away, one 8 GiB extentd; 2 vCPU 4 GiB VMs, postgres:17 fork, pgbench -i -q -s 50 then 60 s -c 4 -j 2). tps: no DAX 1,588/1,842/1,840; DAX 2 GiB 889/976/896; DAX 8 GiB 932/969/336. With synchronous_commit=off: no DAX 4,021/4,700, DAX 2 GiB 2,345-2,723 (still ~1.6x). Per transaction with DAX: 2.25 window MAPs (327 us each as the guest sees them), 0.68 UNMAPs, 0.65 seals (626 us each against 311 without DAX). MAPs by reason per run: evacuate 77,010 (64%), restage 33,233 (28%), grow 4,706, reach 4,705, range 435; window_full only 456. The VMM's 'extentd client' thread: 21.1 s system per run against 3.9 s without DAX (~175 us per MAP): mmap(MAP_FIXED) 38% (unmap_region, flush_tlb_mm_range, smp_call_function_many_cond, kvm_mmu_notifier_invalidate_range_start), KVM_PRE_FAULT_MEMORY 12%, recvmsg 13%; 705k recvmsg, 351k mmap, 638k ioctl, 308k mprotect for 120k MAPs. extentd sends ~6 window-change messages per MAP. External-interrupt exits 472k against 295k. Two of 21 DAX VMs died with KVM_RUN EFAULT, TASK-101's signature (a writable map_own of a newer open extent, then 0.1-1.2 ms later a read-only MAP of an older sealed extent over the chunk), TASK-101's fix not on main. window_largest_free_bytes 'reading -15,908,864' is their per-phase diff of a gauge, not a counter fault. Evidence on their box: ~/fxdax-evidence/daxgap-{default,async,8g,asyncefault}-*, perf data /tmp/fxdax-perf/.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 DAX tps within 10% of no DAX on default pgbench with RustFS 25 ms away, on the sandbox team's setup
- [ ] #2 The VMM's extentd client thread spends under 50 us of host kernel time per MAP
- [ ] #3 30 consecutive DAX VMs, 2 GiB and 8 GiB, with and without synchronous_commit=off, run without an EFAULT
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
