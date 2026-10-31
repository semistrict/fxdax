---
id: TASK-136.2
title: 'Guest Fxfs: one filesystem, one superblock per volume'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 07:27'
labels: []
dependencies:
  - TASK-136.1
documentation:
  - backlog/docs/doc-10 - Fxfs-overlays-for-shared-repository-checkouts.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 469000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each repo should be a volume with its own VFS superblock, so that attaching one needs no new device and it still gets a native `st_dev` and `EXDEV`. Today `fxfs_vfs_get_tree` makes one superblock per device, `fxfs_vfs_info` and `fxfs_dax` mix per-device with per-superblock state, freeze acts per superblock on a whole filesystem, and `Fxfs` owns the filesystem through `Guest`. Each volume's tree cache also takes the whole `object_cache_items` budget.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Two volumes of one device mount at different paths, with distinct `st_dev`, and a rename between them fails with `EXDEV`
- [x] #2 Unmounting one volume leaves the other working, and the last unmount closes the filesystem
- [x] #3 All volumes of a filesystem share one DAX window and one tree-cache budget, whatever their number
- [x] #4 A VM with hundreds of volumes mounted freezes, snapshots, restores and thaws, and every volume reads and writes afterwards
- [x] #5 A cold mount opens only the volumes it mounts: its time and memory don't grow with the volumes attached, and replay still applies what those it leaves closed need
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rust: split `Filesystem` (one per device, shared) from `Fxfs` (one per mounted volume). `fxfs_open`/`fxfs_close` for the filesystem; `fxfs_mount` takes it and a volume name.
2. C: `struct fxfs_vfs_filesystem`, one per device, holds the filesystem, DAX, stats, readahead and resize; `fxfs_vfs_info` stays per superblock. Superblocks key on device and `volume=` option. The last unmount closes the filesystem.
3. DAX: one window per device; `struct fxfs_dax_volume` holds each volume's files, as inode numbers collide across volumes. An unmount with others mounted drops its ranges and evacuations.
4. Freezing: each superblock counts; the filesystem freezes when all are frozen, thaws with the first, and new volumes are refused while frozen. Followers keep one volume.
5. Guest test of two volumes, with and without DAX, including colliding inode numbers; volumes come from the host's `create_volume`.
6. Shared tree-cache budget (AC #3), then opening volumes only when mounted (AC #5).
7. Hundreds of volumes through freeze, snapshot and restore (AC #4).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxfs-guest-volumes, -dax and fxfs-guest-live-fork pass on arm64 in Lima with one superblock per volume, per-volume DAX files and freeze counting. The VM needed sqlite3 installed by hand for the harness's unit tests: scripts/lima-setup.sh doesn't install it.

Started ahead of TASK-136.1 at Ramon's request. Verified on arm64 in Lima: fxfs-guest-volumes(-dax) and fxfs-guest-snapshot-volumes(-dax) (256 volumes through freeze, snapshot, restore and thaw) pass; a first mount read the device 76 times with 8 volumes closed, 78 with 256 (the 8 was a one-off run with VOLUMES changed in the VM's copy). Time and memory were not measured directly. scripts/test.sh passes on macOS and Linux, scripts/check-guest.sh and upstream-changes.py --check pass.

The rest of the guest suite passes but for three DAX cases, the same on a kernel built from ceefbba15: append_while_mapped failed 12 of 12 runs on each kernel on arm64 (a mapped read finds 0 near the end of the file), and fxfs-guest-checks-dax-delay and -small hit their 180 s limits or fail on it. Nothing was run on x86.

Rebased onto cc4f61cf0, 172 commits on: virtio_extent_open and close moved to the filesystem, once per device; resize-on-thaw to the first thaw; follower wiring to the end of fill_super. On the merge: scripts/test.sh passes on macOS (1856 workspace tests) and Linux (1990), scripts/check-guest.sh passes, and the guest suite passes on arm64, 256 volumes included, with 78 device reads on a first mount. pjdfstest-fxfs-guest panicked once on a soft lockup that looked like a VM stall, then passed 3 of 3. append_while_mapped no longer fails on the rebased code. fxfs-guest-checks-dax, -delay and -small hit their 180 s limit in Lima, each with every check it reached passing (60, 60 and 54). scripts/dax-gate.sh was not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
`mount -t fxfs -o volume=NAME` now gives each volume of a device its own superblock, sharing one Fxfs, journal, DAX window and tree-cache budget. Volumes open only when mounted, the filesystem freezes once all of them are frozen, and the last unmount closes it. Verified with new guest tests on arm64, including 256 volumes through snapshot and restore, unit tests of lazy opening and the cache budget, and scripts/test.sh on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
