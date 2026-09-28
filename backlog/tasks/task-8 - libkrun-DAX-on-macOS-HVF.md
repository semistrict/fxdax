---
id: TASK-8
title: libkrun DAX on macOS (HVF)
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 23:53'
labels: []
milestone: m-1
dependencies:
  - TASK-6
type: feature
ordinal: 8000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Guest accesses to the DAX window must reach the kernel's pager: HVF maps host memory into guest physical space, and unmapped or write-protected pages must fault back to fxdax. macOS 26+ supports a 4 KiB IPA granule (hv_vm_config_set_ipa_granule), which lets 4 KiB guest pages map onto the 16 KiB host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A libkrun guest on macOS reads and writes a file through a DAX mapping, with data matching FUSE reads
- [x] #2 Write faults from the guest dirty pages in the kernel and writeback persists them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Kernel port: guest physical aspaces take an optional SecondStage (arch/host/mmu.rs), to which the host MMU hands every change to the window's mappings (map, protect, unmap), as Zircon writes them to the hardware's second stage tables.
2. fxdax: DaxWindow::new / Vfs::create_dax_window take the second stage.
3. libkrun hvf: guest memory fault regions: data and instruction aborts in a registered range go to its handler instead of MMIO; stage-2 map/protect/unmap helpers.
4. libkrun fxdax adapter (macOS): on activation with a shm region, unmap the anonymous shm from HVF, create the window with an HVF second stage, register the window's fault handler, and serve SETUPMAPPING/REMOVEMAPPING.
5. Tests: host MMU second-stage calls (zircon); fxdax Vfs second-stage calls incl. write-protect after writeback; e2e guest test (virtiofs-fxdax) on macOS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Wire libkrun's fxdax adapter setupmapping/removemapping to fxdax::Vfs (TASK-6 API: create_dax_window with the shm size, setup_mapping, remove_mapping) once the shm region is the DaxWindow's host memory and guest faults reach DaxWindow::fault.

- Kernel port: `arch::host::mmu::SecondStage` receives every change to a guest physical window's mappings (map/protect/unmap, runs coalesced on unmap). Deviation documented in hypervisor/aspace.rs and mmu.rs. Upstream writes guest mappings to the hardware's stage-2 tables; here HVF is not relied on to follow host remaps.
- libkrun hvf: `DemandPagedRegion` unmaps the VMM's memory at a range and routes data/instruction aborts there to a handler (new `VcpuExit::DemandPagedMemory`); unresolved faults fall back to MMIO with an error log. `map/protect/unmap_guest_memory` helpers.
- Adapter (macOS): window = shm region; HvfSecondStage maps READ as READ|EXEC so the guest can execute files it maps. Kernel page = 16 KiB = HVF IPA granule, so no 4 KiB granule needed.
- Fault-around: a read fault also maps following resident pages read-only; writeback write-protects clean pages again (tested).
- e2e: virtiofs-fxdax (host formats with fxdax::Vfs, fscks after the run). `cd vendor/libkrun && make test FXDAX=1 BLK=1 NET=1 TEST=virtiofs-fxdax` after a release build with LIBCLANG_PATH/DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/opt/llvm/lib.
- Validation: e2e OK 1/1 on macOS; scripts/test.sh 1739 passed macOS, 1738 Linux; libkrun builds on Linux; krun-hvf tests pass; kernel-cfg --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
HVF guests use fxdax DAX mappings. The DAX window is the virtio-fs shm region, demand paged: HVF passes guest aborts there to DaxWindow::fault, and the kernel's host MMU mirrors each window mapping change into HVF's stage 2 through a new SecondStage hook, so pager faults, dirty tracking and writeback write-protection reach the guest. The virtiofs-fxdax e2e test passes on macOS, including mmap writes, FUSE/DAX coherence and write-back on eviction, with fsck of the image afterwards.
<!-- SECTION:FINAL_SUMMARY:END -->
