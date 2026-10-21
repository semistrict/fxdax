---
id: TASK-42.6.1
title: A read-only mount (-o ro) of a writer's store commits as it unmounts
status: Done
assignee:
  - '@claude'
created_date: '2026-10-21 10:20'
updated_date: '2026-10-21 10:39'
labels:
  - bug
  - followers
dependencies: []
parent_task_id: TASK-42.6
ordinal: 166000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-follow's new check (TASK-42.6 AC1: -o ro writes nothing), GCE debug kernel 2026-10-21: the writer, remounted -o ro and unmounted, moved its store's latest commit from 7 to 8 ('a read-only mount committed: 7 to 8'). Fxfs's close calls Device::close whatever the mount, and the guest's ExtentDevice::close commits unless its device is a follower's (read_only(): the device's read-only feature), not unless its filesystem is mounted read-only; flush alike.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A filesystem mounted read-only on a writable extent device commits nothing, as it runs, flushes or unmounts
- [x] #2 fxfs-guest-follow checks it: the store's latest commit unchanged across the writer's -o ro mount
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentDevice knows when its filesystem writes nothing (writing_nothing, set by Fxfs::mount for a read-only mount; a follower's device from its feature bit); its close and flush then commit nothing. 2. fxfs-guest-follow: the host reads the store's latest commit (StatStore's committed) before and after the writer's -o ro mount and unmount, and fails if it moved.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed: ExtentDevice::writes_nothing (guest/rust/fxfs-linux/src/device.rs), set by writing_nothing() for read-only mounts (fs.rs) and from the read-only feature bit; close and flush skip their commits on it, as they did for followers alone. Verified on GCE: debug kernel fxfs-guest-follow(-dax) 2 rounds of 2, release 10 of 10, each checking the store's latest commit unchanged across the writer's -o ro mount (it moved 7 to 8 before the fix).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A read-only mount of a writer's store committed as it unmounted: the guest's ExtentDevice committed on close unless its device was a follower's. The device now knows its filesystem writes nothing (a read-only mount, or a follower's), and commits nothing then. fxfs-guest-follow checks the store's latest commit across the writer's -o ro mount; it passed 2/2 (debug) and 10/10 (release) on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
