---
id: TASK-136.5
title: 'Attach a base to a sandbox: device read-through to a pinned fork'
status: To Do
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 04:48'
labels: []
dependencies:
  - TASK-136.3
documentation:
  - backlog/docs/doc-10 - Fxfs-overlays-for-shared-repository-checkouts.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 472000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A branch reads only its own and inherited extents (`crates/extent_store/src/state.rs:122`), so a base outside its lineage is unreadable. Ramon chose a device-level read-through to a pinned fork of the base over a second device or a multi-parent graft: one device, one DAX window, no guest DAX change, and no lineage, bucket-layout or collection change.

Only the sandbox manager attaches, through the host control API. The guest has no way to request one. Because a guest names the extent IDs it asks for, everything reachable from an attached base is readable to it, so tenancy rests on what is attached.

This changes doc-4's READ and MAP rule, in the VM attachment. If TASK-96 moves that attachment, it has to carry this.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 After an attach through the host control API, READ and MAP of the base's extents succeed on the sandbox's one device, and extents of bases not attached still fail with `ENOENT`
- [ ] #2 Nothing in the guest protocol can attach a base, shown by a test
- [ ] #3 The sandbox manager is refused an attach of another tenant's private base; a public base's pages are the default arena's and shared with other tenants
- [ ] #4 A pinned generation stays readable after the publisher moves on, and goes once nothing pins it, through ordinary branch deletion
- [ ] #5 VMs mapping one base share its host pages, shown by a test
- [ ] #6 Snapshot, restore and reopening a stopped store keep the attachments
- [ ] #7 doc-4 and the control API documentation describe the change, and the store core (`crates/extent_store`) has no change for it
- [ ] #8 The cost of reading and mapping base extents doesn't grow with attachments, shown with hundreds attached
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
