---
id: TASK-202
title: >-
  Guest Fxfs: a filesystem with a read-only superblock never freezes, so its VM
  can't be snapshotted
status: To Do
assignee: []
created_date: '2026-11-06 06:51'
labels:
  - guest
  - fxfs
  - freeze
dependencies: []
references:
  - guest/linux/fs/fxfs/super.c
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
priority: medium
type: bug
ordinal: 648000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-11-06 review of the guest's Fxfs (review-guest).

doc-10 says "the filesystem freezes once all its superblocks are frozen". `fxfs_vfs_filesystem_join` counts every superblock in `filesystem->volumes`, read-only ones included. But `freeze_super` never calls `->freeze_fs` for a read-only superblock: it marks the superblock SB_FREEZE_COMPLETE and returns 0. So such a superblock never adds to `volumes_frozen`, `volumes_frozen` never reaches `volumes` (super.c `fxfs_vfs_filesystem_freeze`), and `fxfs_freeze` never runs. The device is then never told FREEZE_FROZEN, and the host's `Extent::take_snapshot` refuses the snapshot ("its guest's filesystem is not frozen", vendor/firecracker .../extent/device.rs:468), although every `fsfreeze` the guest ran returned 0. Background work (compactions, the graveyard) also keeps writing. This happens whenever:
- one volume of a writable filesystem is mounted `-o ro` next to writable ones, a configuration fxfs_mount supports on purpose (TASK-160.26); or
- a volume is remounted read-only (`mount -o remount,ro`), which `fxfs_vfs_reconfigure` allows. Afterwards the device can't be snapshotted at all.

The thaw side is symmetrical, since `thaw_super` skips `->unfreeze_fs` for read-only superblocks, so the counts stay consistent. But a filesystem with a read-only superblock can never be snapshotted.

Suggested direction: count only superblocks that can be frozen (writable ones) in what freezing waits for. A read-only superblock of a writable filesystem writes nothing itself. Remount rw<->ro has to move a superblock between the counts (the VFS refuses remounts while frozen). Not covered by a test: guest/sim doesn't build super.c, and fxfs-guest-shared-volumes and fxfs-guest-snapshot mount every volume read-write.

Failing guest check (guest tests are TASK-44.15's): mount volume `b` read-only beside the default volume, freeze both, ask the host for a snapshot. Expect success; main fails with NotFrozen. A second case: remount the default volume read-only, freeze, snapshot.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Freezing every superblock of a filesystem, read-only ones included, freezes the filesystem and tells the device FREEZE_FROZEN
- [ ] #2 A volume remounted read-only, then frozen, freezes the filesystem; thawing thaws it
- [ ] #3 A guest snapshot case with a read-only volume beside a writable one, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
