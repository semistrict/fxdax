---
id: TASK-203
title: >-
  A snapshot of a VM whose window holds private pages records their sources, not
  their bytes
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:54'
updated_date: '2026-11-07 03:48'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the docs pass of 2026-11-06 (doc-4 and doc-13 against the code). The guest says, freezing (fxfs_dax_frozen, guest/linux/fs/fxfs/dax.c:3476-3489), that every private page is rebound to its sealed extent before a snapshot, and that "those it can't, warned of: the device refuses the snapshot". Nothing refuses it: Firecracker's Extent::take_snapshot (vendor/firecracker 96cb97976, src/vmm/src/devices/virtio/extent/device.rs:473-497) checks the device read-only and frozen, forks the store (ExtentStore::fork_quiescent, which checks open extents, not private pages) and records window_mappings(); Window::mappings (crates/fxdax/src/dax.rs:943-957) records each mapping's source, an extent or zeros, never a private page. A snapshot taken with a page still private (a writeback that failed, or a rebind that couldn't be made, the guest only warning) restores that page as its source's old bytes, under a guest whose page cache state says otherwise. doc-4 (Snapshots) and doc-13 now say what the code does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A snapshot of a store whose window holds a private page fails, naming the device and the count, and the VM resumes unchanged; or the private page's bytes are restored with it
- [x] #2 A test takes a snapshot with a private page left (a rebind refused) and shows the refusal or the bytes restored
- [x] #3 dax.c's comment, doc-4 and doc-13 say the same as the code
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find who refuses: the daemon's Fork dispatch refuses a quiescent fork (a snapshot's point) with private pages (e84683ede), before Firecracker's take_snapshot records mappings; extentd's snapshots_after_private_pages_are_rebound_restore_their_bytes tests it. 2. Move the rule into ExtentStore (fork_at_rest: point and fork_quiescent) so every caller is refused, and have window_mappings refuse too (the assertion), describe_mapping keeping an unchecked helper. 3. Test red first at the store. 4. dax.c's comment, doc-4, doc-13. 5. GCE: the DAX snapshot suites.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finding (2026-11-07): a snapshot with private pages left can't lose bytes through fxdax's Firecracker. Its take_snapshot (vendor/firecracker extent device.rs) checks frozen, then forks the store as a point (fork_quiescent, the daemon's Fork with quiescent), then records window_mappings; the daemon's dispatch refused that point with EBUSY while the window held private pages, logging how many (extentd/server.rs, e84683ede, 2026-11-05), so the snapshot failed before anything was made and the VM resumed. extentd's snapshots_after_private_pages_are_rebound_restore_their_bytes (TASK-92.14) shows it: refused with private pages, and once the freeze rebinds them, restored with their bytes. This task, filed by the docs pass of 2026-11-06, read Firecracker and Window::mappings and missed the daemon's check. What was loose: the rule lived in the daemon's request dispatch alone. An in-process ExtentStore made a point with private pages unrefused, and window_mappings listed a window whose private pages it named the sources of. And dax.c's comment said 'the device refuses' where the daemon did. Private pages can be left at a freeze: fxfs_dax_frozen rebinds without writing back (relieve with write false), so pages a writeback kept dirty (TASK-207) or a rebind refused stay private, warned of.

Fix (8023b253b): ExtentStore::fork_at_rest, every point and quiescent fork, refuses with EBUSY while the window holds private pages, its error saying how many ('the DAX window holds N private pages, whose bytes no extent holds: freeze the guest's filesystem, which rebinds them'), checked before the fork's lock; window_mappings refuses the same (the assertion), describe_mapping keeping an unchecked helper (mapped) for its logs; the daemon's own check goes, its dispatch logging the store's refusal with its context as before. No Firecracker change: it takes the point before the mappings, so its snapshot fails 'Extent device ID: cannot fork its store to POINT: EBUSY', naming the device, the daemon's log the count, the VM resuming as it was (AC 1: the device in the VMM's error and the count in the daemon's, as a reply's error carries an errno alone). Test: a_window_holding_private_pages_takes_no_snapshot (point, quiescent fork and window_mappings EBUSY with one private page, 'holds 1 private pages' said; rebound to an extent of its bytes, all made): red without the rule (the point made), green with. extentd's snapshots_after_private_pages_are_rebound_restore_their_bytes and windows_copy_on_write_in_the_vm_process pass through the daemon (Linux, Docker). dax.c's comment, doc-4 and doc-13 say who refuses. Verified: fxdax macOS 288/288; Linux (Docker) 493/494, builds_are_stamped_with_their_commit wanting FXDAX_BUILD_COMMIT, which the container lacked; GCE r840 (us-west1-b, jailed): fxfs-guest-snapshot-dax, -suspend-dax, -snapshot-volumes-dax, -migrate-dax pass. upstream-changes.py --check passes. Every-bug notes: found by the docs pass; blind spot: the refusal was tested through the daemon alone, the store's own API let a point be made; class test: the store-level test, every fork at rest going through one check. Structure: one place now (fork_at_rest) for every point and quiescent fork, and window_mappings can't list a window it can't record.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
No bytes could be lost: the extent daemon already refused a snapshot's point while the window held private pages, before fxdax's Firecracker recorded the mappings (e84683ede, extentd's snapshots_after_private_pages_are_rebound_restore_their_bytes). The rule now lives in ExtentStore (every point and quiescent fork, and window_mappings: EBUSY, how many said), not the daemon's dispatch alone, with a store-level test red without it; dax.c's comment, doc-4 and doc-13 say who refuses. GCE r840: the DAX snapshot suites pass.
<!-- SECTION:FINAL_SUMMARY:END -->
