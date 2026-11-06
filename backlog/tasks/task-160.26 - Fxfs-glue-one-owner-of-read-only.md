---
id: TASK-160.26
title: 'Fxfs glue: one owner of read-only'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 06:56'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 597000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Read-only lives in FxFilesystem's options, the guest's Filesystem.read_only, fxfs-posix's read_only, ExtentDevice.writes_nothing and DEVICE_READ_ONLY; flush_volume checks the filesystem's, not the volume's. Explains TASK-42.6.1. Proposal: derive them from one at open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One source of read-only
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. FxFilesystem's options own read-only. fxfs-posix's Fxfs::new derives a volume's from its mount's and its filesystem's, the one place it is decided; the guest's Filesystem drops its own field for the filesystem's options.
2. flush_volume checks the volume's (Fxfs::read_only), which covers a -o ro mount of a writable filesystem's, not only the filesystem's.
3. The guest's ExtentDevice: writes_nothing is set by Filesystem::open alone (writing_nothing), not also from DEVICE_READ_ONLY at find; read_only() renamed follows(), what the flag says; Device::is_read_only reports writes_nothing.
4. Test: fxfs-posix a_volume_is_read_only_as_it_or_its_filesystem_is. Guest: cargo check of fxfs-linux for the kernel target (Lima).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on task-160-24 (2e4531954). The owner: FxFilesystem's options (read_only). Derived from it, at one place each: fxfs-posix Fxfs::new (a volume is read-only if its mount asks or its filesystem is); the guest's Filesystem::read_only() reads the filesystem's options, its copy gone; the guest's ExtentDevice writes nothing only as Filesystem::open says (writing_nothing), which computes read_only = asked || follows once, no longer also initialised from DEVICE_READ_ONLY at find; ExtentDevice::read_only() is follows(), what the feature bit means; Device::is_read_only reports writes_nothing. flush_volume checks the volume's read-only (Fxfs::read_only), so a volume mounted -o ro on a writable filesystem refuses it: it checked the filesystem's only, the shape TASK-42.6.1 came from.
How found / blind spot: the structure review (TASK-160); no test asked a volume's read-only of a read-only filesystem whose mount didn't say so. Test: fxfs-posix tests::a_volume_is_read_only_as_it_or_its_filesystem_is, red with Fxfs::new taking its argument as it is (left [false, true], right [true, true]), green with the fix. Looked around: the other read-only checks (evict, import, graveyard, object_manager, allocator, journal) read the filesystem's options or the volume's already.
Runs: as TASK-160.24's (same batch); fxfs-linux type-checks in Lima. Not run: GCE guest cases (fxfs-guest-follow covers a -o ro mount of a writer's store).

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: FxFilesystem's options own read-only; Fxfs::new derives a volume's (fxfs-posix lib.rs:556); a_volume_is_read_only_as_it_or_its_filesystem_is passes in r691.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A volume's read-only is derived from its filesystem's, at one place (TASK-160.26, 2026-11-05)**

FxFilesystem's options own read-only. fxfs-posix's `Fxfs::new` makes a volume read-only if its mount asks or its filesystem is; the guest's `Filesystem` reads the filesystem's options rather than keeping a copy; its `flush_volume` checks the volume's, which refuses a flush of a volume mounted -o ro on a writable filesystem (it checked the filesystem's only). The guest's `ExtentDevice` writes nothing only as `Filesystem::open` says (`writing_nothing`), no longer also from DEVICE_READ_ONLY at `find`; `read_only()` is `follows()`, what that flag says.
**Instead:** derive the filesystem's read-only from its device (`Device::is_read_only`) in `FxFilesystemBuilder::open`, which changes upstream code.
**Undoing it:** the guest's field back, and `Fxfs::new` taking its argument as it is.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Read-only has one owner, the filesystem's options; a volume's, the guest's and ExtentDevice's are derived from it; flush_volume checks the volume's. On main: 0460570cf. Verified: a_volume_is_read_only_as_it_or_its_filesystem_is in GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
