---
id: TASK-164.9
title: >-
  Copy-on-write DAX: the iomap that dirties a page makes it private, one owner
  of a page's state
status: To Do
assignee: []
created_date: '2026-11-04 14:33'
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
- [ ] #1 Dirty implies private by construction: one transition function, and no path marks a page dirty without it
- [ ] #2 Writeback never depends on the kernel's DAX dirty tags or entry locks
- [ ] #3 specs/dax-cow models the new rule; guest/sim's cow seeds and scenarios pass; the three bugs' mutants stay caught or become unbuildable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
