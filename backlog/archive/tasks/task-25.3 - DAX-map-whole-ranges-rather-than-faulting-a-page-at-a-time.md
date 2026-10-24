---
id: TASK-25.3
title: 'DAX: map whole ranges rather than faulting a page at a time'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-24 00:53'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 28000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reading a cold file through a DAX window runs at 172 MiB/s, against 1.6 GiB/s without DAX (docs/measurements/gce-fs-bench-2026-10-10T1823Z.md): each guest fault on the window goes through userfaultfd (on Linux) to the in-process kernel, which maps the faulting page and at most 16 pages already present (PAGE_FAULT_MAX_OPTIMISTIC_PAGES), and the pager reads 128 KiB at a time. Map what the file has in memory when the guest sets up a mapping, and fault in larger runs, reading ahead, so that a cold read costs a fault per range rather than per few pages. Related: TASK-12 (host mappings per page).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cold sequential reads through DAX at least as fast as without DAX, measured on GCE
- [x] #2 Warm DAX reads no slower
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure where a cold DAX read's time goes on GCE (KVM's exits and faults per second, fxdax's fault thread's counters). 2. Map the pages a file has in memory at FUSE_SETUPMAPPING (ZX_VMAR_OP_MAP_RANGE). 3. Map runs of physically consecutive pages with one host mapping and one second stage call. 4. Have KVM map read-only window pages for the guest as fxdax maps them (KVM_PRE_FAULT_MEMORY on a vCPU of the window's own, in libkrun). 5. Cut the per-run cost of mapping at FUSE_SETUPMAPPING (longer runs). 6. Measure 3 iterations against no DAX.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Diagnosis (gce-fs-bench-2026-10-10T1953Z): a cold read cost ~25 µs a page: ~7 µs fxdax's userfaultfd faults (every one a write: KVM faults pages in for writing), ~17 µs KVM's own fault on each page, which nested virtualization makes slow. Step 2 (2518f59): mmap random reads 13k -> 667k/s. Step 3 (368a4d3): ~13 pages a host mapping; cold seq 197 -> 270 MiB/s, cold random reads 8.3k -> 31.8k/s. Step 4 (8bbf381, libkrun 6b5dfac): KVM's faults while reading fell from ~60k/s to ~1k/s. 3 iterations (T2010Z): cold seq 795 MiB/s (no DAX 1,652), cold random 27.9k/s, mmap random reads 1.11M/s (ext4 1.2M), mmap random writes 4.6k/s. Random writes 4,560/s against 5,453 in the baseline, to check.

Step 5 (6f8998f): map_range hands the host MMU up to 512 pages a call: cold seq 795 -> 862 MiB/s, cold random reads 27.9k -> 35.9k/s (T2016Z, 3 iterations). Warm DAX reads unchanged (10.5 GB/s; AC #2). AC #1 not met: 862 MiB/s against ~1,650 without DAX. Where the rest goes (T2019Z): FUSE_SETUPMAPPING ~580 µs a 2 MiB range, mostly KVM's pre-faults; the other ~1.6 ms of the ~2.2 ms a range takes is the guest's, the transport's and L0's per-page faults under nested virtualization. Next: TASK-25.8 (pre-fault after replying; 2 MiB host pages).

Obsolete (2026-10-23 audit of stale tasks, the user agreeing): fxdax's FUSE/virtio-fs server and files' DAX windows were removed in fda9d7d2, fxdax-extentd became the only store server in 537c7eb4, and libkrun went with TASK-40 (5ba39497). Guest Fxfs over the virtio extent device replaced them; its benchmark against ext4-dax-pmem continues under TASK-26.8 and TASK-91/92. FUSE_SETUPMAPPING is gone with it.
<!-- SECTION:NOTES:END -->
