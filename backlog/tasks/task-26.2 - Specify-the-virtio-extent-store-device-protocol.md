---
id: TASK-26.2
title: Specify the virtio extent-store device protocol
status: To Do
assignee: []
created_date: '2026-10-13 19:53'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: task
ordinal: 38000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's Fxfs and the host's extent store talk only through this device (doc-2): extents allocated by ID, mapped into a DAX window, freed, and made durable in order by commits that do not wait for upload. Fork, object storage and resuming elsewhere (later milestones) must fit without changing it incompatibly.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A specification in the backlog defines the device's queues, shared-memory region, every request and reply (ALLOC, FREE, MAP, UNMAP, COMMIT, ROOT, and WRITE if kept), errors, and ordering and durability semantics
- [ ] #2 It states how branches, forks and immutable extents (doc-2's provisional decisions) appear to the guest, if at all
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
