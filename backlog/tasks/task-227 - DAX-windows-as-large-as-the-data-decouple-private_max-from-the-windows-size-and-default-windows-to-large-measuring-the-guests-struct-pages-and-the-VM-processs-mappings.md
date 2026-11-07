---
id: TASK-227
title: >-
  DAX windows as large as the data: decouple private_max from the window's size
  and default windows to large, measuring the guest's struct pages and the VM
  process's mappings
status: To Do
assignee: []
created_date: '2026-11-07 08:14'
updated_date: '2026-11-07 09:41'
labels: []
dependencies: []
priority: high
type: enhancement
ordinal: 663000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user (2026-11-07): the window is address space, not memory; it can be as large as the host's memory, mapped again in every VM, one copy of each page. Firecracker's virtio-pmem maps a whole image (its file's size) this way.

What keeps fxdax's windows small today:
- private_max is half the window (TASK-164.11), reserved against the tenant's memory: a 64 GiB window would reserve 32 GiB per VM. Decouple it: a fixed or tenant-configured cap (1-2 GiB), independent of the window's size.
- Defaults: the window is a device config value (dax_window_size, 1 GiB in tests and the perf gate).

What a large window costs, to measure: the guest's struct pages, 64 B per 4 KiB page (about 1.6%), made a part at a time as touched (TASK-92.8), so only for what's reached; the VM process's mappings against vm.max_map_count (uffd budget); KVM's page tables for mapped pages; boot time (about 23 ms per GiB of window before TASK-92.8; re-measure).

TASK-136.1 (r870) shows the cost of a small window: a 1.54 GB tree reread through a 1 GiB window and 1 GiB of daemon memory took 25.6 s against 0.6 s from the guest's page cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 private_max is independent of the window's size (a fixed default or per-tenant setting), and the reservation against tenant memory follows it; forked pgbench keeps forked-dax at or above 0.9
- [ ] #2 Windows default to large (e.g. the host's memory or a configured maximum), in the VMM and the tests, and a 64 GiB or larger window boots and runs the DAX suites
- [ ] #3 Measured on GCE: guest struct-page memory, VM-process mappings and boot time against window size (1, 8, 64 GiB and more), and the Linux checkout's warm reread with a window larger than the tree
- [ ] #4 README, doc-13 and the perf gate updated
- [ ] #5 Target (the user, 2026-11-07): the Linux checkout's warm full-tree reread on Fxfs DAX is as fast as ext4 on Firecracker's virtio-pmem (mounted -o dax) or faster, same host, same VM size; both measured in the same run (linux-checkout cases, TASK-136.1)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07, TASK-136.1's r872, the AC #5 baseline: one n2-standard-8, VMs of 4 vCPUs and 4 GiB, an 8 GiB window and 6 GiB of daemon memory. Warm full-tree reread of the Linux checkout (1.54 GB, 91k files), checkout VM / fresh VM: ext4 on virtio-pmem with dax=always (linux-checkout-ext4-pmem) 729 / 712 ms; Fxfs DAX 801 / 751 ms (1.10 / 1.05 of pmem); Fxfs DAX sandboxes over a shared base 759-781 ms. Without DAX: ext4 on virtio-blk 763 / 766 ms, Fxfs 858 / 791 ms. The gap to pmem is the size of Fxfs's gap to ext4 without DAX, likely per-file open and lookup cost (not profiled). Cold read in a fresh VM: Fxfs DAX 5.6 s, pmem 8.8 s. Raw output in docs/measurements/linux-checkout-2026-11-07/r872, tables in docs/measurements/linux-checkout-2026-11-07.md. To rerun: FXDAX_LINUX_CHECKOUT_WINDOW=8589934592 FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY=6442450944 FXDAX_E2E_TEST='linux-checkout-ext4-pmem linux-checkout-fxfs-dax' scripts/test-gce.sh.
<!-- SECTION:NOTES:END -->
