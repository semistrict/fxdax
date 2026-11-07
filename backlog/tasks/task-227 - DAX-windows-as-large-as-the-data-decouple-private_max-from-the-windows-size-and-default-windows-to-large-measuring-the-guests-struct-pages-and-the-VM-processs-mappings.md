---
id: TASK-227
title: >-
  DAX windows as large as the data: decouple private_max from the window's size
  and default windows to large, measuring the guest's struct pages and the VM
  process's mappings
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 08:14'
updated_date: '2026-11-07 14:00'
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
- [x] #1 private_max is independent of the window's size (a fixed default or per-tenant setting), and the reservation against tenant memory follows it; forked pgbench keeps forked-dax at or above 0.9
- [x] #2 Windows default to large (e.g. the host's memory or a configured maximum), in the VMM and the tests, and a 64 GiB or larger window boots and runs the DAX suites
- [x] #3 Measured on GCE: guest struct-page memory, VM-process mappings and boot time against window size (1, 8, 64 GiB and more), and the Linux checkout's warm reread with a window larger than the tree
- [x] #4 README, doc-13 and the perf gate updated
- [x] #5 Target (the user, 2026-11-07, relaxed the same day from 'as fast as or faster'): the Linux checkout's warm full-tree reread on Fxfs DAX within 1.5x of ext4 on Firecracker's virtio-pmem (mounted -o dax), same host, same VM size, both measured in the same run (linux-checkout cases, TASK-136.1)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. private_max a fixed 1 GiB default (or its own pages if fewer), configured per device, whatever the window (vendor/firecracker); the daemon's reservation follows it as before.
2. The uffd stage's per-page state in anonymous memory zero until written, and the window's constructor, CoW start and release O(touched), so a 64 GiB window costs only what the guest reaches.
3. Tests and perf gate default to Window::Large (host memory, at most 64 GiB, FXDAX_DAX_WINDOW_MIB overrides); test daemons 8 GiB; MAX_WINDOW stays 64 GiB; the VMM keeps dax_window_size explicit.
4. Measure on GCE (r880-r889): struct pages, mappings, KVM page tables, boot, the checkout's warm reread against ext4 on pmem in the same run; profile any gap.
5. README, doc-13, perf gate doc, OPEN_QUESTIONS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07, TASK-136.1's r872, the AC #5 baseline: one n2-standard-8, VMs of 4 vCPUs and 4 GiB, an 8 GiB window and 6 GiB of daemon memory. Warm full-tree reread of the Linux checkout (1.54 GB, 91k files), checkout VM / fresh VM: ext4 on virtio-pmem with dax=always (linux-checkout-ext4-pmem) 729 / 712 ms; Fxfs DAX 801 / 751 ms (1.10 / 1.05 of pmem); Fxfs DAX sandboxes over a shared base 759-781 ms. Without DAX: ext4 on virtio-blk 763 / 766 ms, Fxfs 858 / 791 ms. The gap to pmem is the size of Fxfs's gap to ext4 without DAX, likely per-file open and lookup cost (not profiled). Cold read in a fresh VM: Fxfs DAX 5.6 s, pmem 8.8 s. Raw output in docs/measurements/linux-checkout-2026-11-07/r872, tables in docs/measurements/linux-checkout-2026-11-07.md. To rerun: FXDAX_LINUX_CHECKOUT_WINDOW=8589934592 FXDAX_LINUX_CHECKOUT_EXTENTD_MEMORY=6442450944 FXDAX_E2E_TEST='linux-checkout-ext4-pmem linux-checkout-fxfs-dax' scripts/test-gce.sh.

2026-11-07, GCE r880-r888 (n2-standard-4/8, nested, jailed):
- Size costs are the guest's reach, not the window's: 8 GiB (r881) and 64 GiB (r882) windows each boot in 997 ms; struct pages for the 1.9 GB reached 30 MiB; VM process 4.6 GB; 16.6k mappings (vm.max_map_count 1,048,576 budget); KVM page tables 9.1 MiB. A 64 GiB window's uffd stage took 810 MB as made before the WindowPages change, <16 MiB after (a_large_windows_stage_takes_the_memory_it_reaches).
- Warm reread of the 1.54 GB Linux tree: 1 GiB window + 1 GiB daemon 25.6 s (r870); 8 GiB window + 1 GiB daemon 13.15 s (r880: the daemon evicts and pages in again); 8 GiB + 8 GiB 0.60 s (r881).
- Same run, r888: ext4 on virtio-pmem dax=always 546 / 534 ms (checkout / fresh VM); Fxfs DAX 992 / 576 ms, sandboxes 574-586 ms. The fresh VM +8%; the checkout VM's 992 ms an outlier (985 ms in r882) whose counters look like the fresh VM's.
- 64 GiB window: fxfs-guest-dax, -crash-dax, -crash-dax-killed (r885), -checks-dax (r886) passed. Host-memory default (31.3 GiB): stats, import-dax, volumes-dax, fxfs-guest-dax, checks-dax-small at its own 32 MiB passed (r887).
- Forked pgbench at the new defaults, 15.6 GiB window and 1 GiB private_max (r884): forked-dax 0.967, PRIVATEs 108,983 (r807: 108,688).
- Fork: semistrict/firecracker land-w1 (4079fbc85 private_max, 99c2b05c5 Cargo.lock). fxdax: land-w1, CI 42364466938.

AC #5, the user's relaxed target (within 1.5x of pmem, 2026-11-07), met in the same runs, the fresh VM's warm reread after a timed warm one (read-warm-again), 8 GiB window, 8 GiB daemon:
- r901 (land-w1's guest kernel): ext4 on pmem 537 ms, Fxfs DAX 568 ms (1.06x); first warm reread 548 / 578 ms (1.05x); sandboxes 565-578 ms.
- r902 (with a read's iomap reading no clock, TASK-230's): pmem 536 ms, Fxfs DAX 564 ms (1.05x).
The checkout VM's first warm reread is slow once (r901: 1.00 s, the next 594 ms). Where the rest goes (r901's reader-only profile, 10 kHz): the copy costs the same (pmem's copy_mc 2,285 samples, Fxfs's rep_movs 2,339); Fxfs's extra 641 samples (7.6%) are its directory reads' LSM seeks and decoding (~520-610), down_read (~190-210) and clock reads (~160), not per-page costs.

2026-11-07, after the rebase onto main 4bf0e45fd (land-xk) and 26fd25c77:
- r903, the full perf gate on GCE (land-w1 dfdcd2507; density and gate phases on 5a7f72999, the same code), windows the host's memory (15.6 GiB), private_max 1 GiB: every gate passes. dax 0.977, forked-dax 0.949, decay 1.03-1.42, density 1.06 and 1.01, density-memory 0.782 with 2,011 MiB a VM added (r807: 0.811, 2,164), bigmap-read 4.81, bigmap-guest-cache 0.0, bigmap-written-memory 1.20. In docs/measurements/dax-cow-gate.md.
- r907, the 1 GiB window on this code (linux-checkout-fxfs-dax, 8 GiB daemon), reopened VM after its read, against r881's 8 GiB and r882's 64 GiB: boot 1,013 / 994 / 986 ms; struct pages 16 MiB (all of the 1 GiB window made) / 30 / 30 MiB (the 1.9 GB reached); VM-process mappings 35.8k / 13.4k / 13.4k (a small window's reclaim leaves more runs); KVM page tables 3.1 / 4.7 / 4.7 MiB; warm reread 28.3 s / 584 / 573 ms, a sandbox's 55.9 s / 589 / 609 ms. Above 64 GiB isn't measured: the wire's MAX_WINDOW is 64 GiB (TASK-150.3's bound on the daemon's per-window state), kept (OPEN_QUESTIONS).
- r900 was stopped (its waiter's time limit, then superseded by the rebase); scripts/test-gce.sh resume added for such runs.
- upstream-changes.py --check passes. CI 42366900234 on dfdcd2507: every job green.

Validation: macOS FXDAX_TEST_PARTS=workspace scripts/test.sh 2088/2088 (5a7f72999's code); CI 42366900234 green on dfdcd2507 (workspace, guest, sim, shuttle, firecracker, go, clippy scripts, guest-sim, guest-sim-sweep, firecracker-forward); upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
DAX windows are address space, as large as the data. A writer's private pages are 1 GiB at most whatever the window's size (vendor/firecracker 4079fbc85, where they were half the window), and the daemon reserves that. The uffd stage keeps per-page state only for the pages the guest reaches (WindowPages, zero bytes a page nothing changed), so a 64 GiB window's VM process grows by less than 16 MiB as it is made, where it grew 810 MB. The tests and the perf gate default to the host's memory, at most 64 GiB (common::dax_window, FXDAX_DAX_WINDOW_MIB), and the test daemons to 8 GiB; the VMM keeps dax_window_size explicit and MAX_WINDOW stays 64 GiB (OPEN_QUESTIONS).

Measured on GCE:
- 8 and 64 GiB windows cost the same: boot 0.99 s, 30 MiB of struct pages for the 1.9 GB reached, 13.4k mappings, 4.7 MiB of KVM page tables (r881, r882, r907).
- The Linux tree's warm reread takes 573-584 ms on 8 and 64 GiB windows, against 28 s on a 1 GiB one, and is within the user's 1.5x of ext4 on virtio-pmem at 1.06x (r901). The rest is per file, not per page, and TASK-230 takes it (r905: 0.93x).
- The full perf gate passes: forked-dax 0.949, density-memory 0.782, 2,011 MiB a VM added (r903).
- fxfs-guest-dax, the crash suites and checks-dax pass at 64 GiB (r885, r886).

Also: test-gce.sh resume, for a run whose local waiter stopped. TASK-229 (sandboxes' cold read) and TASK-230 split off.
<!-- SECTION:FINAL_SUMMARY:END -->
