---
id: TASK-202
title: >-
  Guest Fxfs: a filesystem with a read-only superblock never freezes, so its VM
  can't be snapshotted
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:51'
updated_date: '2026-11-06 09:40'
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
- [x] #1 Freezing every superblock of a filesystem, read-only ones included, freezes the filesystem and tells the device FREEZE_FROZEN
- [x] #2 A volume remounted read-only, then frozen, freezes the filesystem; thawing thaws it
- [x] #3 A guest snapshot case with a read-only volume beside a writable one, failing on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. super_operations freeze_super/thaw_super wrapping the VFS's, settling a read-only superblock's frozen state with the filesystem (fxfs_vfs_settle_frozen).
2. Reconfigure: sync on rw->ro; settle on ro->rw. Rust freeze/thaw of a read-only Fxfs: the device alone.
3. Guest check read_only_freezes (remounted ro; mounted ro); snapshot cases fxfs-guest-suspend-read-only-volume and -remounted-read-only.
4. Verify as TASK-199, bug back.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on guest-199, commit aa7073516 (with TASK-200).

Fix: Fxfs's super_operations freeze_super and thaw_super call the VFS's, then fxfs_vfs_settle_frozen, under s_umount, makes the filesystem's count follow a read-only superblock's VFS state (the VFS freezes one without freeze_fs): the filesystem freezes once every superblock is frozen, read-only ones too, as doc-10 says, which covers a filesystem whose every superblock is read-only (remounted read-only). A failed filesystem freeze thaws the superblock again; a failed thaw keeps the filesystem frozen (TASK-200), and a FITHAW of the VFS-thawed superblock retries it. A volume remounted read-only is synced first (ext4 and XFS do), since freeze_super doesn't sync a read-only superblock; a remount writable of a superblock counted frozen thaws the filesystem first. A filesystem opened read-only freezes by closing the device and telling it (Freezer) alone. Followers' read-only superblocks aren't counted (their device can't be snapshotted; freezing would stop their batches). Rejected: counting writable superblocks only (an all-read-only filesystem could then never freeze). Limit, recorded in OPEN_QUESTIONS.md: an emergency thaw (do_thaw_all) calls no filesystem's thaw_super, so a read-only superblock it thaws stays counted frozen until frozen and thawed again, remounted writable, or unmounted.

Blind spot: every snapshot case mounted its volumes read-write, and guest/sim doesn't build super.c. Tests added: check read_only_freezes (default volume remounted read-only, then the filesystem mounted read-only: each FIFREEZE and FITHAW tells the device, its freeze requests counted, and what was written reads back; a guard remounts writable if it fails) and cases fxfs-guest-suspend-read-only-volume (a volume mounted read-only beside the default, frozen with it, snapshotted, restored, thawed, read) and fxfs-guest-suspend-remounted-read-only (the default remounted read-only, frozen, snapshotted, restored, thawed, remounted writable, written). Bug back (GCE r763, hooks removed): both cases fail with the device refusing the snapshot ('its guest's filesystem is not frozen'), read_only_freezes fails (device told 0 times).

Verified: macOS workspace green; Linux Docker (privileged) 2221/2225, the 4 Docker-only failures known on main; check-guest.sh arm64 ok; upstream-changes.py --check ok; GCE us-west1-b jailed, r762: workspace 2225/2225, fxfs-guest-checks, -checks-dax, -checks-dax-small, fxfs-guest, -dax, -snapshot-dax, -suspend-dax, -migrate-dax, -stats, -follow-remount, -suspend-read-only-volume, -suspend-remounted-read-only all pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Read-only superblocks' freezes reach the filesystem through freeze_super/thaw_super hooks; a remount read-only syncs; a read-only Fxfs freezes the device alone. Verified by read_only_freezes and two new snapshot cases, red with the bug back (GCE r763: NotFrozen), green on r762.
<!-- SECTION:FINAL_SUMMARY:END -->
