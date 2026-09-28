---
id: TASK-6
title: 'virtio-fs server: DAX setupmapping and removemapping'
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 04:44'
labels: []
milestone: m-0
dependencies:
  - TASK-5
type: feature
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With DAX the guest maps file pages directly instead of copying through FUSE read/write. The server answers FUSE_SETUPMAPPING/REMOVEMAPPING by mapping ranges of a file's pager-backed VMO into the DAX window, so guest loads and stores fault into the Fxfs pager and dirty tracking.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 setupmapping maps a file range into the DAX window read-only or read-write as requested; removemapping unmaps it
- [x] #2 Stores through a mapping mark pages dirty and reach disk on fsync, shown by a test
- [x] #3 Access to a mapped range past end of file behaves as the guest expects (documented and tested)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fuchsia.io File.GetBackingMemory in the stand-ins (upstream handler and flag checks); FxFile::get_backing_memory upstream, returning the file's own VMO since the kernel has no VMO children.
2. crates/fxdax dax.rs: DaxWindow over the kernel's GuestPhysicalAspace; files' VMOs mapped into its root VMAR with SPECIFIC_OVERWRITE; guest faults resolved with fault().
3. Vfs::create_dax_window / setup_mapping / remove_mapping, keeping mapped files' connections open.
4. Tests: reads, stores dirtying a clean page and reaching disk on fsync, upgrade to writable, partial removal, past-EOF behavior, errors, unlinked mapped file.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Past EOF (documented in dax.rs): the guest reads zeros there (files' VMOs are unbounded); stores past the end in the last page never reach the file, as Fxfs writes back only up to its end and zeroes the rest of the last block. Linux's FUSE client bounds accesses by i_size itself. Tested by dax_past_the_end_of_the_file.
- libkrun's adapter still answers SETUPMAPPING/REMOVEMAPPING with ENOSYS: until TASK-8/9 make the device's shm region the window's memory, mapping would lose guest stores.
- Validation: scripts/test.sh macOS 1737 passed, Linux 1736 passed; libkrun builds with fxdax.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax serves DAX mappings: Vfs::setup_mapping/remove_mapping map files' pager-backed VMOs (via fuchsia.io GetBackingMemory) into a DaxWindow, the kernel's guest physical address space, whose faults go through Fxfs's pager and dirty tracking. Tested through the window's host memory, including stores reaching disk on fsync and past-EOF behavior. Verified: 1737/1736 tests pass on macOS/Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
