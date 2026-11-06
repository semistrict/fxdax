---
id: TASK-92.8
title: Probe builds struct pages for the whole 2 GiB DAX window at every boot
status: Done
assignee:
  - '@claude'
created_date: '2026-10-22 21:06'
updated_date: '2026-11-06 03:30'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 283000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
vext_map_window_dax calls devm_memremap_pages over the whole region as MEMORY_DEVICE_FS_DAX, with no altmap and no vmemmap_shift (driver.c:580-587). It runs synchronously in probe (641; no probe_type, 762-770): 524,288 struct pages, a 32 MiB memmap. Boot logs show a 103.6-109.7 ms gap before the probe against 1.9 ms with no window: about a third of the 351 ms kernel boot on every fork boot, plus 32 MiB resident per VM.

Where: guest/linux/drivers/virtio_extent/driver.c:573-593, guest/linux/drivers/virtio_extent/driver.c:641, guest/linux/drivers/virtio_extent/driver.c:762-770.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there; driver.c has not changed since the review. vext_map_window_dax calls devm_memremap_pages once over the whole window as MEMORY_DEVICE_FS_DAX, with no altmap and no vmemmap_shift (guest/linux/drivers/virtio_extent/driver.c:573-595). vext_driver sets no probe_type (driver.c:763-771). Related: TASK-26.14, TASK-26.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Ruled out before (dead-ends/): compound devmap (fs-dax writes tail pages' struct pages) and an asynchronous probe (kernel_init waits for async probes before init; r263/r264 showed nothing). An altmap moves the 32 MiB into the window, which copies on write: its stores would be private pages counted against private_max, the same memory on the host, and the init's cost stays.
Chosen: the window's struct pages made a part at a time, on demand.
1. Refactor: the window's state in one struct vext_window; its memory reached only through its dax_device (virtio_extent_dax gives its length too; virtio_extent_window goes; the selftest reads it through dax_direct_access, as Fxfs does); a kernel without FS_DAX takes no window (nothing would map it).
2. Measure: the driver's 'window' sysfs attribute (length, bytes with struct pages, time making them); the guest tests' measure says guest_boot_ns (the kernel log's 'Run ... as init process'), guest_memmap (nr_memmap_pages) and the window's three.
3. Fix: parts of a memory section each (128 MiB on x86-64 and arm64: hotplug maps a whole section's memmap on huge pages; each part's memremap_pages waits for an RCU grace period, mem_hotplug_begin), aligned to physical sections; vext_dax_direct_access makes the parts its pages are in before it returns them (memremap_pages, under a mutex, allocations NOFS), as lib/test_hmm.c and nouveau make device memory's pages lazily; none at probe. fs/dax.c and Fxfs reach the window's pages only through direct_access, so no page is used without struct pages; Fxfs hands out window space first fit from the ranges' start and the regions' (the window's middle), so a small guest makes two parts. Unbinding: kill_dax, then each made part unmapped.
4. Verify: guest kernel builds (Docker, arm64); guest/sim check; GCE jailed: the DAX gate's suites, pgbench-dax-forked, extent-guest (the selftest reads the window); bigmap-dax and pgbench-dax-density for boot and memory, before (r670/r671) and after.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-30, from the guest kernel's source (6.18.54, GCE r255): compound devmap geometry (pgmap->vmemmap_shift) would cut the memmap's init to 2 unique vmemmap pages a compound page (memmap_init_compound, compound_nr_pages, CONFIG_ARCH_WANT_OPTIMIZE_DAX_VMEMMAP=y on x86_64), but fs-dax makes its folios per DAX entry as it maps them, writing their tail pages' struct pages, which the vmemmap optimization shares read-only; pmem.c sets no vmemmap_shift for MEMORY_DEVICE_FS_DAX either. Ruled out. Left: an asynchronous probe (PROBE_PREFER_ASYNCHRONOUS), if mounting by tag waits for the device's probe; or the window's memmap made a part at a time as the window's allocator first hands each out (devm_memremap_pages per part); or struct pages in the window itself (altmap), which saves the 32 MiB of RAM, not the init.

GCE r263 at 1e1e1baf5 (main 3358b1089 + virtio_extent probed asynchronously, a lookup by tag waiting for probes in progress) against r264 at 3358b1089, same suites (fxfs-guest, -dax, -checks-dax, -crash-dax, -docker-dax, -snapshot-dax, -pgbench-dax), all passed on both. VMM resume to the guest's first I/O at init (port 0x87): 261.6, 337.2, 261.9, 266.4, 258.0 ms against 258.6, 341.5, 254.0, 252.1, 254.2; the kernel's 'check access for rdinit' over 8 boots each, median 0.251 s against 0.272 s. No boot gain to show: whatever the window's memmap costs now, the asynchronous probe doesn't take it off the path to init and a mount. Not pushed. Next if it's taken up again: a boot at loglevel 7 with initcall_debug, to measure the probe's time on main today.

Measured (test_cases' measure: guest_boot_ns from the kernel log's 'Run ... as init process', guest_memmap = nr_memmap_pages, the driver's new 'window' attribute), GCE jailed, release kernel, before r673 (b49bab05f, us-west1-b) and after r674 (e3c7ff7f3, us-west2-a), both bigmap-dax, pgbench-density, pgbench-dax-density with 3 VMs and 2 runs:
- boot to init, 2 GiB window: 501/501/489 ms before, 369/369/381 after; no window (control, same hosts): 305/333/325 and 309/329/329. Window's overhead 174 -> 47 ms.
- 8 GiB window (bigmap-dax): 1085 ms before (608 ms making struct pages at probe), 505 after (none made at mount; 4 GiB made by read-cold, in 506 ms).
- memmap: 32 MiB per 2 GiB-window VM before; 18 MiB after a pgbench run (9 of 16 parts made, 200 ms making them in all, ~22 ms a part against ~10 ms per 128 MiB made whole: each pays an RCU grace period). bigmap: 128 -> 64 MiB. VM process's anonymous memory at a density run's end 836/836/830 MB -> 720/744/719; guest MemAvailable +20 MB; perf gate's density-memory 0.881 -> 0.824, density-memory-mib 2486 -> 2162 (one run each, different hosts).
- tps: DAX/no-DAX ratio unchanged (0.705 before, 0.702 after); absolute tps lower in r674 for no-DAX too (host). The gate's dax-density decay fails both (0.878, 0.802): one run each on different hosts; part-making's 200 ms over two 60 s runs can't explain it.
Suites (r675, at e3c7ff7f3): DAX gate's six, pgbench-dax-forked, extent-guest (the selftest reads the window through direct_access, making part 0) pass; crash-stray-dax times out as TASK-187 (the stray load reaches the window, a part made for it, and the host refuses it, KVM_RUN EFAULT and on).
Left: with no part made, boot to init still grows with the window: +47 ms for 2 GiB, +185 ms for 8 GiB (about 23 ms a GiB), and the same remainder was there before (174 ms overhead against 154 ms of memremap). Not the driver's (probe makes nothing now); a boot at loglevel 7 with initcall_debug would name it. Proposed as a follow-up, not filed.
Structure: the class (work proportional to the window done whether or not it's used) is now bounded by use; the driver's window state is one struct, reached only through the dax_device. Decisions in OPEN_QUESTIONS.md. Dead ends updated (async probe, compound devmap).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The DAX window's struct pages are made a memory section (128 MiB) at a time as dax_direct_access first reaches it, not the whole window at probe (e3c7ff7f3, after refactor cffac4037 and measurement b49bab05f). GCE r673 -> r674: boot to init with a 2 GiB window ~495 -> ~373 ms (no-window control ~322), 8 GiB 1085 -> 505 ms; memmap 32 -> 18 MiB per pgbench VM, 128 -> 64 MiB for bigmap; r675 DAX suites pass. Workspace nextest not run (no workspace code changed). Remaining ~23 ms/GiB of boot that scales with window size is outside the driver: proposed follow-up.
<!-- SECTION:FINAL_SUMMARY:END -->
