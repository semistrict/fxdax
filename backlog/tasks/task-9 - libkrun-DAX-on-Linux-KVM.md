---
id: TASK-9
title: libkrun DAX on Linux (KVM)
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 05:00'
labels: []
milestone: m-1
dependencies:
  - TASK-6
type: feature
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On KVM the DAX window is host memory registered as a guest memory slot; guest faults on pages the pager has not supplied must be caught (userfaultfd or equivalent) and routed to the kernel's pager, and write-protect faults must mark pages dirty.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A libkrun guest on Linux reads and writes a file through a DAX mapping, with data matching FUSE reads
- [ ] #2 Write faults from the guest dirty pages in the kernel and writeback persists them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Wire libkrun's fxdax adapter setupmapping/removemapping to fxdax::Vfs (TASK-6 API: create_dax_window with the shm size, setup_mapping, remove_mapping) once the shm region is the DaxWindow's host memory and guest faults reach DaxWindow::fault.

After TASK-8: the adapter creates the DAX window only on macOS (FxdaxFs::open). On Linux, KVM follows host mappings, so the memslot could be the window's host reservation (DaxWindow::host_base) with no SecondStage; guest accesses to unmapped window pages then fault in KVM (EFAULT/KVM_EXIT_MEMORY_FAULT) and must reach DaxWindow::fault. virtiofs-fxdax e2e test exists (TASK-8); run it in Lima.
<!-- SECTION:NOTES:END -->
