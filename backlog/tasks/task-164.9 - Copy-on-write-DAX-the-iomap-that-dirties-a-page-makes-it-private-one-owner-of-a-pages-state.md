---
id: TASK-164.9
title: >-
  Copy-on-write DAX: the iomap that dirties a page makes it private, one owner
  of a page's state
status: Done
assignee: []
created_date: '2026-11-04 14:33'
updated_date: '2026-11-05 04:54'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 622000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decided by the user on 2026-11-04. A page's state lives in three places that change at different moments: dax.c's dirty and private bitmaps, the kernel's DAX entry (its dirty tag and lock), and the host's private page. guest/sim found three bugs of that split (writeback leaving a faulting page writable as its DAX entry was locked; a stale dirty tag leaving a range unreclaimable; writeback taking pages the host hadn't made private), each fixed locally in batch 2026-11-04d. Restructure so the class can't recur: the iomap that dirties a page makes it private on the host before any store can land, so dirty implies private; writeback works from dax.c's dirty set alone and write-protects by page tables, never relying on the kernel's dirty tags or entry locks; one function owns each transition (clean -> dirty+private -> written back -> rebound).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Dirty implies private by construction: one transition function, and no path marks a page dirty without it
- [x] #2 Writeback never depends on the kernel's DAX dirty tags or entry locks
- [x] #3 specs/dax-cow models the new rule; guest/sim's cow seeds and scenarios pass; the three bugs' mutants stay caught or become unbuildable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One owner of a copy-on-write page's state (aa923012c): the iomap that dirties a page makes it private, so dirty implies private by construction; writeback works from dax.c's dirty set and write-protects by page tables, not the kernel's DAX dirty tags; guest/sim's host refuses a store to a page no PRIVATE made private; specs/dax-cow's DirtyImpliesPrivate. Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
