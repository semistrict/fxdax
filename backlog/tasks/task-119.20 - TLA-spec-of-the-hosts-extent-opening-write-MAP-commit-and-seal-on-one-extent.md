---
id: TASK-119.20
title: >-
  TLA+ spec of the host's extent opening: write, MAP, commit and seal on one
  extent
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 23:39'
updated_date: '2026-10-28 23:54'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 460000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's DAX protocol has TLA+ specs (specs/dax, specs/dax-region); the host's handling of an extent's lifecycle has none. TASK-121's last loss was in it: ExtentStore::write's check that an extent wasn't open and its write to the store were two steps, which a writable MAP opening the extent from the store could come between; the commit then wrote the open extent's memory, zeros, over the write. A spec with each lock's critical section one step finds such interleavings in seconds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 specs/extent-open models write (to an open extent's memory or the store), MAP (read-only or writable, opening the extent from the store), and commit (writing open extents' memory to the store and sealing them), on extents written by several writers
- [x] #2 Its invariant: every write that returned is in what the commit after it wrote; TLC finds no violation with the rules as main has them
- [x] #3 A mutation splitting write's check from its store write, TASK-121's bug, is found by scripts/check-tla.sh
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. specs/extent-open/ExtentOpen.tla: extents in the store (absent, open-in-store with data, sealed), the host's open map (memory per open extent), writers (write: check open, then the VMO or the store), a mapper (MAP writable or read-only: under the open lock, opening from the store), a committer (under the open lock: store := memory of each open extent, all sealed, open drained). Data as sets of the writes' ids.
2. Rule atomicWrite: write's check and store write one step (main). Invariant NoLostWrite: every write that returned is in the store's extent once a commit after it ends.
3. ExtentOpen.cfg with the rules; mutations/SplitWrite.cfg without atomicWrite, expect NoLostWrite.
4. Register in scripts/check-tla.sh's header list; run it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
scripts/check-tla.sh (all models): extent-open/ExtentOpen holds, 423 distinct states; extent-open/SplitWrite violates NoLostWrite. Its counterexample is TASK-121's: Write(a) checks not open, MapWritable opens the extent empty, WriteStore(a) returns with the write in the store alone. One extent, two writers, a writable and a read-only MAP, the guest's store, two commits. DoD #1 not run for this task: it changed no Rust (a spec and check-tla.sh's header); the fxdax crate's tests passed on Linux at cae97000a.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/extent-open/ExtentOpen.tla models the host's ExtentStore opening one extent: writes (memory or store), read-only and writable MAPs opening it from the store, the guest's store, commits sealing it. NoLostWrite: every returned write is in what the extent reads. Holds with writeUnderLock (main, cae97000a); mutations/SplitWrite (TASK-121's split check and store write) is found by scripts/check-tla.sh, the counterexample being TASK-121's interleaving.
<!-- SECTION:FINAL_SUMMARY:END -->
