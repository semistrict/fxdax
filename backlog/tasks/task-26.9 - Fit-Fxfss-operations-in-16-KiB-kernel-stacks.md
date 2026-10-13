---
id: TASK-26.9
title: Fit Fxfs's operations in 16 KiB kernel stacks
status: To Do
assignee: []
created_date: '2026-10-13 22:13'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-3 -
    TASK-26.1-spike-porting-Fxfs-into-the-guests-Linux-kernel.md
parent_task_id: TASK-26
ordinal: 45000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Linux kernel threads have 16 KiB of stack on x86-64 and arm64, and the system call and VFS use some before a filesystem is called. Optimized, Fxfs's metadata operations take 2-7 KiB, but formatting takes 35, mounting with journal replay 34 and journal compaction 25 (docs/measurements/fxfs-stack-2026-10-13, crates/fxdax/examples/fxfs-stack.rs). The cause is depth, not single large futures: call chains are 15-25 async functions deep, and each poll frame reserves room to build the futures it awaits, so frames such as Flusher::finish reach 14 KB. An experiment (branch stack-experiment) boxed 25 functions' futures in non-inlined wrappers, with no caller changes and one allocation per call: format 35->19 KiB, replay 34->25, compaction 25->16, close 5->10. The remaining deep chains were not located: a debugger stops on the in-process kernel's own page faults, so a tool that reports the deepest call chain is needed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-stack can report the call chain, with frame sizes, at the deepest point of a phase
- [ ] #2 Every phase fxfs-stack measures, including format, mount with replay, journal compaction and close, takes at most 12 KiB below the executor in optimized builds on arm64 and x86-64
- [ ] #3 Each change to Fxfs is a single attribute or line where it occurs, with the deviation documented, and Fxfs's tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
