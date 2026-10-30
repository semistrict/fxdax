---
id: TASK-131.2
title: >-
  fsync without a seal: commit without write-protecting and evacuating, sealing
  only at fork, snapshot or reclaim
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-30 00:05'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 473000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: DAX seals cost 0.54 ms against 0.32 without DAX, almost all ~3 window remaps; if fsync could commit without write-protecting and evacuating, and seal only at fork, snapshot or reclaim, DAX writes could match no-DAX. TASK-112.3 (doc-9) answered round 7b: every fsync must seal, write-once extents underpinning durability by whole extents, sharing by ID and the shared page cache; a durable-but-open staging extent costs store-format, protection and seal-on-share changes. Revisit with their numbers: which of the seal's remaps are on the commit path now, and what a durable open extent would take.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A design answer with this round's numbers, and the remaps left on the commit path measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured, GCE r261 at 9601cd2ba (nested n2, release kernel, 1 VM, 5 x 60 s pgbench), extentd's side: DAX 176108 commits at 120 us each (commit-ns 21.1 s): seal 44 us (remap 18.7, populate 10.2, cached 2.9, supply 1.8), write 49 us, store 18 us, wait 8 us. No DAX 346967 commits at 288 us each, 246 us of it waiting (commit-wait-ns 85.3 s). The seal's remaps cost DAX 3.3 s of the run; its 1078678 window MAPs cost 73.6 s (68 us each) and the VMM's 6265407 stage operations 127.2 s (20 us each, 5.8 per MAP). So dropping the seal from fsync would save at most ~4% of DAX's daemon time; the window maps are what to cut (TASK-131.1).

Correction, the guest's side (r261 /proc/fs/fxfs stats): the guest sees a seal at 587 us (seals_ns 103.1 s / 175724; device commit 577 us), not extentd's 120 us; without DAX 408 us (device commit 407 us, of which extentd's 288). So ~457 us of each DAX commit is outside extentd's commit handling, against ~119 us without DAX: the request waiting behind window MAPs and the VMM's stage operations (127 s of its time over the 300 s run). Evacuation, which follows each seal: 199289 evacuations, 506714 of the guest's 779119 window maps (65%, 7.7 GB), 105.5 s of MAP time and 183.4 s with a file's semaphore held for writing. So the seal itself is cheap at extentd; what costs is the evacuation after it and the stage-op volume both cause. Next: fewer stage ops per map (TASK-131.1), then whether evacuation can be skipped or deferred for pages rewritten soon.
<!-- SECTION:NOTES:END -->
