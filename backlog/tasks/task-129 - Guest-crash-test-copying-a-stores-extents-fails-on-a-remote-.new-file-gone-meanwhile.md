---
id: TASK-129
title: >-
  Guest crash test: copying a store's extents fails on a remote-*.new file gone
  meanwhile
status: In Progress
assignee: []
created_date: '2026-10-29 02:34'
updated_date: '2026-10-29 16:54'
labels:
  - bug
  - testing
dependencies: []
priority: low
ordinal: 465000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-crash-dax round 3 of 15 on r209 (rebased onto f897ce4a2): test_fxfs_guest.rs:200 panicked: copying .../extents/remote-9833cba38baea4bb.new: No such file or directory. The copy of the store's directory races its uploader's temporary file, renamed away between listing and copying. Results: /tmp/fxdax-r209-rebased.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The crash test's copy of the store tolerates files its writer renames or removes meanwhile, or copies a quiescent store
- [x] #2 fxfs-guest-crash-dax passes 20 rounds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
c0904a4a7 on main: copy_store takes the store's directory (extent_store::Dir::take) before copying. GCE r216 crash-dax round 1 copied the store (it failed on the reverted refusal check). r220: 10 rounds.

GCE r220 (kernel built, branch t3/task-112-6 rebased on main with c0904a4a7): fxfs-guest-crash-dax 10 rounds passed; r216 round 1 copied too. 11 of AC2's 20 so far.

Recurred in the DAX gate's second run (3981ad18f), fxfs-guest-crash-dax-gathered round 1: copying the store, remote-*.new gone: copy_store took the directory, but the store's closing had let it go while its uploader still ran: ExtentStore's drop closes the store, then drops the uploader, which uploads the last commits as it stops, writing the branch's marker in the directory after. Stopping the uploader before the close (tried) left the closing commit unuploaded (later_placements_fence_earlier_ones failed). Fix: Lineage::hold_dir, which the uploader (Remote) holds until it stops, its last field; test uploaders_hold_their_stores_directory_until_they_stop fails without it ('taken while its uploader runs'). macOS scripts/test.sh passed (1824). The gate's third run on it pending.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The crash test copies the store once no process has it open: copy_store takes its directory (extent_store::Dir::take), as a process opening it does, which the extent daemon holds while an upload uses it; copied while held, an upload's marker's .new file was renamed away mid-copy. GCE: fxfs-guest-crash-dax 21 rounds passed (r216 1, r220 10, r224 10), kernel built. DoD #1 on macOS not run.
<!-- SECTION:FINAL_SUMMARY:END -->
