---
id: TASK-79.107
title: >-
  Guest DAX: a fault's keep=true map_write writes through the device into a
  staging extent the window may not have opened yet
status: Done
assignee: []
created_date: '2026-10-24 14:08'
updated_date: '2026-10-29 06:42'
labels:
  - bug
dependencies: []
parent_task_id: TASK-79
priority: medium
ordinal: 381000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found bisecting TASK-79.11 (2026-10-24): having txn_map_write write a fault's page into its moved block through the device killed the VM in fxfs-guest-checks-dax (Firecracker: vcpu run: Bad address). fxfs_dax_iomap_write passes keep=true when it can't allocate the held copy, and txn_map_write then writes the old blocks' contents into the new ones the same way: device WRITEs to the staging extent, which may not be open in extentd yet (the restage maps it after a seal, in the background), so the write goes to the store and not the open extent's memory the window maps. Likely the same failure, on a rare path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test drives a fault's keep=true path (the held allocation failing, by fault injection) and shows the failure
- [x] #2 Device writes to a staging extent before the window opens it are coherent with the window's later writable mapping
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-99 (2026-10-24): forcing the keep=true path (fxfs.dax_fail_held) stopped VMs on EFAULT; the cause was a race in region_open it made common (fixed in TASK-99), not incoherent device writes: extentd opens an extent written through the device from the store's contents. faults_unheld now covers the keep path; AC1 may be met by it.

2026-10-29: both met by later work. AC1: faults_unheld (TASK-119.18) drives the keep=true path with fxfs.dax_fail_held; it failed on TASK-99's region_open race, since fixed, and passed 10 rounds each of fxfs-guest-checks-dax and -dax-delay at TASK-121's close (70c2ea556). AC2: the incoherence was real, on the host: ExtentStore::write checked the extent wasn't open, let go of the lock, then wrote the store; a writable MAP opening it between read the store without the write, and its commit wrote zeros over it. cae97000a writes under the open extents' lock; extents.rs's race suite (writes_racing_the_mapping_that_opens_their_extent_are_committed and the two after it, 2000 rounds each, TASK-119.19) and specs/extent-open (TASK-119.20) cover it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Device writes into a staging extent before the window opens it are coherent with its later writable mapping since cae97000a (TASK-121): the store write happens under the open extents' lock, so a MAP opening the extent reads the store before the write or after it. Covered by extents.rs's race suite and specs/extent-open; the keep=true path itself by faults_unheld. No new code. DoD #1 not run for this closure.
<!-- SECTION:FINAL_SUMMARY:END -->
