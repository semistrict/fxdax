---
id: TASK-79.74
title: >-
  Legacy Filesystem: a read or write under way as its volume unmounts isn't
  waited for: a write may be lost, a page request after may hang
status: To Do
assignee: []
created_date: '2026-10-23 04:40'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 330000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.57's review points (round 1). Unmounting a volume of the legacy virtio-fs Filesystem (crates/fxdax/src/filesystem.rs) now waits for every operation that runs its work on Fxfs's executor (Volume::run holds a guard on the volume's scope), but reads and writes copy to and from a file's pages through its zx::Stream on the caller's thread, under no guard. A write under way as the unmount begins may land after FxVolume::terminate's last flush (LastChance), whose force_clean forgets dirty pages, so the write is lost. A read or write that needs a page request once the scope has finished may hang: Fxfs's Pager::spawn drops the request when try_active_guard fails. Neither is shown by a test yet; both come from reading the code. Filesystem::unmount's doc says reads and writes are not waited for.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A write that returned success before an unmount completed is in the volume after it is mounted again, or the write fails, pinned by a test
- [ ] #2 A read or write racing an unmount completes or fails; none hangs, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
