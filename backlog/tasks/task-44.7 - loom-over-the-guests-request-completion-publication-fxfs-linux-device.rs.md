---
id: TASK-44.7
title: loom over the guest's request completion publication (fxfs-linux device.rs)
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 136000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/rust/fxfs-linux/src/device.rs publishes a request's completion with hand-written orderings (status stored Relaxed, done stored Release, both loaded after an Acquire of done) and the capacity with Acquire/Release: the one kind of code where loom, which models C11's weak orderings, finds what Shuttle (all atomics SeqCst) can't (doc-8, Why Shuttle, not loom). Small, so a loom test is cheap, if device.rs's completion can be built for the host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A loom test exhausts the interleavings of a completion's publication against its poll and its waker, and of capacity updates against readers
- [ ] #2 Mutation checks: the done store made Relaxed, or the status load moved before the done load, makes the loom test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
