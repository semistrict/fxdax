---
id: TASK-119.6
title: >-
  crash-dax in the default set, crashing at a random point, with overwrites of
  synced data
status: In Progress
assignee: []
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 19:43'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 442000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crash-dax crashes at one fixed point, only writes new files with write(2), never overwrites synced data, and is not in the default set.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 In the default e2e and GCE sets
- [x] #2 Random crash point; workload includes overwrites of synced ranges, mmap stores with msync, truncate
- [x] #3 After the crash every fsynced state survives; every unsynced byte is old or new
- [ ] #4 A variant kills the VMM with SIGKILL
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29: fxfs-guest-crash and -crash-dax are in test-e2e.sh's and test-guest-kernel.sh's defaults and the DAX gate (AC1). The crash point is random each round (FXDAX_CRASH_MS); the workload fsyncs new files, overwrites synced pages through write(2)+fsync and through a mapping+msync, and now truncates and extends a file, each size synced, the pattern written where it extends (AC2); after the crash every file whose fsync returned is whole, each rewritten page holds one of its writes since the last synced, and the resized file is as large as its last synced resize or one after, holding the pattern or an unsynced extension's zeros (AC3). On main: the resizing writer; writers quieted before the stop drains the console (r247: they kept it, and the VM, going); the kernel's lines taken out of the guest's output before it is parsed (r248: a stop's line split a synced resize's report). GCE r249: fxfs-guest-crash and -crash-dax 10 rounds each passed; r248 crash-dax and -gathered 8 each. AC4 (a variant killing the VMM with SIGKILL) not done.
<!-- SECTION:NOTES:END -->
