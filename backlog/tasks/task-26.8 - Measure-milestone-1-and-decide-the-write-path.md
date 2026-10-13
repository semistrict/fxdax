---
id: TASK-26.8
title: Measure milestone 1 and decide the write path
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
labels: []
milestone: m-3
dependencies:
  - TASK-26.4
  - TASK-26.6
  - TASK-26.7
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: task
ordinal: 44000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Milestone 1's purpose (doc-2) is to show metadata at ext4 speeds with guest-owned metadata, and to settle the open write-path decision: writing extents through the DAX window with commit barriers, or with WRITE requests.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 GCE runs measure the fs benchmark and git status on guest Fxfs against ext4 in the guest, recorded in docs/measurements
- [ ] #2 Both write paths are measured, and the decision is recorded in doc-2 with the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
