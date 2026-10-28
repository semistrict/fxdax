---
id: TASK-112.1
title: >-
  Guest DAX: a first read of base pages already in extentd memory runs ~100 MB/s
  (~40 us per 4 KiB page), no faster than no DAX
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 02:30'
updated_date: '2026-10-28 03:30'
labels:
  - bug
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 424000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox density test (TASK-112), VMs 2-8 read 445 MB of base files with every page already in extentd memory in 4.5 s, as fast as no DAX, whose reads copy through extentd; the second read takes 0.22 s. Static DAX on virtio-pmem pays only KVM first-touch per page (their estimate 10-20 us nested).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Where a first DAX read of base pages already in extentd memory spends its time is measured and recorded (guest fault, MAP, VMM window change, KVM)
- [x] #2 A first read of 445 MB of base files already in extentd memory, in a second VM, takes at most half of no DAX on GCE nested
- [x] #3 A test or bench in the repo measures it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure: bench-fork-fxfs-guest-dax-warm reads /usr (464 MB) first and again, with the guest's window counters and requests in the log (fork-read-usr, fork-reread-usr).
2. Found (n2 nested): first read 2.31 s, second 0.09 s; 222 reach MAPs of ~2 MiB, 0.11 s in all. The rest is the hypervisor's first touch of each page, ~19 us/page, since 1cd30919 stopped pre-faulting read-only maps. Pre-faulting them again: 0.69 s.
3. Fix: virtio-extent F_MAP_AHEAD / MAP_AHEAD (doc-4): the guest sets it on maps it reads next (reach, range), not evacuations; the VMM's worker, after the MAP, pre-faults the installed pages of read-only ones (RemoteExtentStore::prefault -> UffdSecondStage::prefault_installed).
4. Verify: fork-warm bench on GCE n2; pgbench-dax 2 GiB unchanged or better; Linux workspace and DAX guest suites.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Bench at 7c9b7ceab+ (n2, r201): fork-read-usr 2.31/2.31/2.32 s, fork-reread-usr 0.09; c3 (r200) 2.23-2.25 / 0.09. Same tree with every read-only map pre-faulted (r202): 0.67-0.70 s / 0.09.

MAP_AHEAD (r201-112c, n2 nested): fork-read-usr 0.727/0.722/0.716 s (2.31 before), reread 0.09; the read's 222 reach MAPs now 902 us each with their pre-fault (0.20 s in all). pgbench-dax 2 GiB unchanged: 1118/1021/971/916/938 (before 1124/1015/974/920/953). DAX suites fxfs-guest-dax, fxfs-guest-checks-dax, extent-guest, fxfs-guest-pgbench-dax pass; Linux workspace 2662 passed (pages_asked_for_ahead_are_prefaulted_as_installed); Firecracker's extent device tests 9 passed (vmm --features fxdax, FXDAX_EXTENTD_REMOTE=memory:///, FXDAX_EXTENTD_TOKEN=1). AC2 (at most half of no DAX) not measured here: the bench has no fork case without DAX; the sandbox's no-DAX first read was 4.3-4.5 s for 445 MB.

r201-112r (n2 nested, 3 iterations, medians): first read of /usr (464 MB) on a fork of a warm base: Fxfs DAX 0.687 s, Fxfs without DAX 2.653 s (new case bench-fork-fxfs-guest-warm), ext4 dax=always on virtio-pmem 2.051 s, ext4 on pmem 4.138 s; second read 0.092 / 0.030 / 0.081 / 0.029 s; pgbench -S 27.2k / 21.5k / 23.9k / 23.1k tps. AC2 met: 0.69 s against no DAX's 2.65.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A first read of base data was the hypervisor faulting each page on first touch (~19 us nested), since 1cd30919 stopped pre-faulting read-only maps; the read's 222 MAPs were 0.11 s of 2.31 s. virtio-extent MAP_AHEAD (doc-4, F_MAP_AHEAD bit 8, flag bit 2): the guest sets it on maps it reads next (reach, range), and the VMM pre-faults the installed pages of read-only ones once mapped (RemoteExtentStore::prefault, UffdSecondStage::prefault_installed), leaving evacuations unprefaulted. GCE n2 nested: first read of /usr 0.69 s (2.31 before; no DAX 2.65; ext4 dax=always on virtio-pmem 2.05), second 0.09 s, pgbench-dax unchanged. Test pages_asked_for_ahead_are_prefaulted_as_installed; Linux workspace 2662 passed; Firecracker extent device tests 9 passed; DAX guest suites pass. Commits ff7961411, Firecracker 12b14e485.
<!-- SECTION:FINAL_SUMMARY:END -->
