---
id: TASK-109
title: >-
  Sandbox team, round 6 (2026-10-25 report): DAX about 45% slower than no DAX
  under pgbench at b746104
status: To Do
assignee: []
created_date: '2026-10-25 23:26'
updated_date: '2026-10-26 03:14'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sandbox retest at 139e82b (TASK-101 landed; 2026-10-25 20:06-21:02 EDT): 32 traced DAX VMs, 0 crashes (b746104: 2 of 21): 12 x 2 GiB (init 18.1-19.7 s, 944-1,041 tps, no downward trend over the 12), 10 x 8 GiB (17.5-18.2 s, 957-1,047 tps, one run each, so run 3's collapse untested), 10 x 2 GiB synchronous_commit=off (2,239-2,689 tps). One EFAULT in all (82k-360k KVM_RUN returns per VM), a fault-in that recovered within 1 ms. AC3 met in substance (no VM lost; one transient EFAULT, retried). extentd's refused-store counter still passed 131,072 (KVM's async page-fault worker storing to read-only pages, TASK-102's design); extentd's memory stayed 3.4-7.6 GB, never at its 8 GiB cap. The DAX gap (AC1, AC2) is unchanged: TASK-91.16 in progress.

Our comparison, 2026-10-25 22:10-22:45 EDT, GCE n2-standard-8 us-east4-a, PostgreSQL 17, pgbench -i -q -s 50 then 5 x 60 s -c 4 -j 2, shared_buffers=256MB (branch at cac2afe77, TASK-91.16 in): native on the host in a container held to 2 CPUs/4 GiB (scripts/pgbench-native.sh): load 11.2 s, 3,287/3,561/3,565/3,293/3,684 tps; native uncapped: 7.6 s, ~3,440 (commit-latency bound at -c 4); fxdax no-DAX in Firecracker (fxfs-guest-pgbench, bucket 25 ms away): 16.7 s, 1,693/1,730/1,707/1,697/1,739 (~49% of native); DAX: 18.8 s, 995/923/903/834/848 (~53% of no-DAX, falling 15% over the runs). ext4 on virtio-blk in the bench's VMs pending.

Bench VMs (2 vCPU, 2 GiB, Postgres defaults, no bucket latency, scale 50, 60 s, 3 iterations): ext4 on virtio-blk 1,140 tps (init 11.8 s), fxfs no-DAX 1,660 (11.4 s), fxfs DAX 405 (21.6 s): filed as TASK-109.3.
<!-- SECTION:NOTES:END -->
