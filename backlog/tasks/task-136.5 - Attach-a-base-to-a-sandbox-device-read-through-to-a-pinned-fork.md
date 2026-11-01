---
id: TASK-136.5
title: 'Attach a base to a sandbox: device read-through to a pinned fork'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 16:04'
labels: []
dependencies:
  - TASK-136.3
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
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
- [x] #1 After an attach through the host control API, READ and MAP of the base's extents succeed on the sandbox's one device, and extents of bases not attached still fail with `ENOENT`
- [x] #2 Nothing in the guest protocol can attach a base, shown by a test
- [x] #3 The sandbox manager is refused an attach of another tenant's private base; a public base's pages are the default arena's and shared with other tenants
- [x] #4 A pinned generation stays readable after the publisher moves on, and goes once nothing pins it, through ordinary branch deletion
- [x] #5 VMs mapping one base share its host pages, shown by a test
- [x] #6 Snapshot, restore and reopening a stopped store keep the attachments
- [x] #7 doc-4 and the control API documentation describe the change, and the store core (`crates/extent_store`) has no change for it
- [x] #8 The cost of reading and mapping base extents doesn't grow with attachments, shown with hundreds attached
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentStore: routes by branch, each base opening on first read; READ and read-only MAP through it.
2. Durable record beside the store, copied by forks before their commit.
3. Daemon: one instance of each generation; AttachBase on the control API, for live and stopped stores; tenancy by directory; MakePublic's generation flag.
4. Tests: unit (extents.rs), daemon (tests/bases.rs), protocol (requests_attach_nothing). doc-4, doc-10, proto.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decisions: routes by branch, excluding the store's own ancestry; a base opens when first read, so hundreds attached cost a line and map entries each; the record is a file beside the store, copied by a fork before its commit; a non-public base attaches only within its tenant's directory; MakePublic's generation flag marks one branch, so a publisher's store stays writable. Limits: a stopped store doesn't keep a generation from being deleted, and then reads ENOENT of it; attachments are local to the host. Host-level proof only: tests/bases.rs on a real daemon, unit tests with DAX windows, requests_attach_nothing. No guest has read through yet (TASK-136.4).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
AttachBase attaches a pinned fork to a store, live or stopped: READ and read-only MAP of what it holds read through, by a lookup of the extent's branch, and forks and restored VMs keep it. A non-public fork attaches only within its tenant, a public generation is shared as one copy in the default arena, and a generation stays through what its store frees. crates/extent_store is unchanged. Verified with crates/fxdax/tests/bases.rs on a real daemon, unit tests in extents.rs with 300 attached, and requests_attach_nothing, sending every request with the base's path; scripts/test.sh on macOS and Linux. Not yet exercised by a guest.
<!-- SECTION:FINAL_SUMMARY:END -->
