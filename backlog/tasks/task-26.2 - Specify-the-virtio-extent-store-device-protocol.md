---
id: TASK-26.2
title: Specify the virtio extent-store device protocol
status: Done
assignee:
  - '@claude'
created_date: '2026-10-13 19:53'
updated_date: '2026-10-13 22:18'
labels: []
milestone: m-3
dependencies: []
documentation:
  - backlog/docs/doc-4 - Virtio-extent-store-device-specification.md
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
- [x] #1 A specification in the backlog defines the device's queues, shared-memory region, every request and reply (ALLOC, FREE, MAP, UNMAP, COMMIT, ROOT, and WRITE if kept), errors, and ordering and durability semantics
- [x] #2 It states how branches, forks and immutable extents (doc-2's provisional decisions) appear to the guest, if at all
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Survey how Fxfs uses its device: offsets in extent records, superblocks at fixed offsets, barriers (PRE_BARRIER), trim, flush.
2. Decide the addressing model (asked the user, who was unsure; took the sparse address space, recording the alternative).
3. Write the specification as doc-4: features, configuration, queues, shared memory, every request, errors, ordering and durability, and how branches, forks and immutability appear (they don't).
4. Update doc-2 to follow it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The user was unsure between a sparse address space and allocation-level extents; the spec takes the sparse address space and records the alternative, revisitable since addresses encode extent and offset. Fxfs needs no ALLOC round trips and keeps its offsets and fixed superblocks, so doc-2's ALLOC and ROOT are gone. Forks, branches and immutability are host-side (copy-on-write per branch, immutable versions), invisible to the guest. No code changed: the DoD's test suites and upstream check passed at 76a461f, and main's code is unchanged since.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Specified the virtio extent-store device in doc-4: a sparse per-filesystem address space (storage used where written, released by FREE), READ/WRITE/FREE/MAP/UNMAP/COMMIT/INFO over multiqueue virtio with a DAX window, barrier epochs with prefix-at-a-barrier crash semantics and COMMIT not waiting for durability unless asked, errors, and how forks and immutable versions stay host-side. doc-2 updated to match.
<!-- SECTION:FINAL_SUMMARY:END -->
