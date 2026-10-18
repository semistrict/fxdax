---
id: TASK-44.3
title: Shuttle over extentd's DAX window and userfaultfd remapping
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 132000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd's DAX window (crates/fxdax/src/dax.rs, dax/uffd.rs, the window in extents.rs) resolves faults concurrently, as futures, on fault threads, while the extent store maps and remaps pages; a remap takes several system calls, between which a vCPU's access fails with EFAULT and is retried (Firecracker's vCPU retry). uffd.rs has one test. Its system calls are behind a trait, SecondStage, so a fake second stage can run the real code under Shuttle (doc-8 testing 2 explains Shuttle and why not loom).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fake SecondStage and the window's real code run under Shuttle (fuchsia-sync and the fault futures on Shuttle's primitives with a shuttle feature), with faults (reads, writes, a write after a read) arriving between every step of maps, remaps to read-only, unmaps and moves
- [ ] #2 Invariants checked: a fault resolves to the page the window maps now, or fails with EFAULT only inside a remap and succeeds on retry; no store goes untracked (a page written is dirty); no page of zeros where data was
- [ ] #3 Mutation checks: each ordering rule of the remap (register before map, write-protect before read-only, ...) turned off makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
