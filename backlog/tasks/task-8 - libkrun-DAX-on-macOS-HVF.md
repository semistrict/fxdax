---
id: TASK-8
title: libkrun DAX on macOS (HVF)
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
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
- [ ] #1 A libkrun guest on macOS reads and writes a file through a DAX mapping, with data matching FUSE reads
- [ ] #2 Write faults from the guest dirty pages in the kernel and writeback persists them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
