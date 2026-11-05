---
id: TASK-44.16
title: Guest DAX tracepoints and a trace checker of the protocol's invariants
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-05 03:05'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 395000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's beliefs about its pages and the host's mappings are each logged in part; nothing checks them against each other over a run. Tracepoints at the protocol's steps, merged with extentd's log of MAP, UNMAP and PRIVATE, let an offline checker replay the protocol's state machine (or the kernel's RV monitors run it live) and name the first step that breaks an invariant.

At 9f81f6018, after writable DAX was deleted (37c331a6c), the protocol is doc-13's copy-on-write: no region is open in the window, nothing is evacuated, and no page is writable through the window except a private one. The steps to trace: MAP of a sealed extent or zeros, read-only (fxfs_dax_map_pages, guest/linux/fs/fxfs/dax.c:836); PRIVATE, before a page is dirty (fxfs_dax_dirty, dax.c:2421-2480; fxfs_device_private, device.c:273); writeback's write-protect and its write through fxfs_write_many (fxfs_dax_write_back, dax.c:2085; fxfs_dax_protect, 2054-2069); the seal (fxfs_device_seal_begin/end, device.c:167-192); rebind (fxfs_dax_rebind, dax.c:2168); UNMAP. The invariants are specs/dax-cow's (DaxCow.tla), among them DirtyImpliesPrivate. dax.c's trace ring (fxfs.dax_trace, dax.c:518-600) records a page's maps and iomaps, not the host's side.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The guest emits tracepoints at the DAX protocol's steps, and extentd logs its MAPs with the same offsets
- [ ] #2 A checker replays a run's merged trace against the protocol's invariants and reports the first violation
- [ ] #3 A run with a refused store (guest/sim/mutants/task-164.4-protect-by-entries.patch put back on a guest: a fault's writable PTE kept across a writeback, the host refusing its store after the rebind), replayed, is reported at its first broken invariant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sandbox team round 7 (TASK-111) asks for tracepoints or USDT probes at seal start and end, MAP by reason, evacuation, guest fault to extentd, and window reclaim, so bpftrace can measure latencies.
<!-- SECTION:NOTES:END -->
