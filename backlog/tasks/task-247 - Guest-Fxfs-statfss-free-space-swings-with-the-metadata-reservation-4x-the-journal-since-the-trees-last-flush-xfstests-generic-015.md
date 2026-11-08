---
id: TASK-247
title: >-
  Guest Fxfs: statfs's free space swings with the metadata reservation, 4x the
  journal since the trees' last flush (xfstests generic/015)
status: To Do
assignee: []
created_date: '2026-11-08 08:51'
labels:
  - fxfs
  - testing
dependencies: []
priority: low
ordinal: 681000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
xfstests' generic/015 fails on xfstests-fxfs-guest and -dax (GCE r945, TASK-149.25): on a 256 MiB scratch filesystem it fills with dd, removes the file, syncs, and expects df's free space back within 1% of where it began; it is 256692 KiB, short of 258346..263565. Cause: fxfs-posix's statfs (crates/fxfs-posix/src/lib.rs, usage()) reports free = blocks - allocator.get_used_bytes(), and used_bytes counts reservations (allocator.rs ByteTracking/used_bytes), among them the object manager's metadata reservation, kept at required_reservation(): the largest compaction reservation + reserved_space_from_journal_usage(journal bytes since the earliest unflushed) = 4x those bytes (object_manager.rs:39, :119). A sync writes the journal, not the trees, so the reservation stays grown; with fxfs-linux's reclaim size of capacity/32 (8 MiB on 256 MiB, fs.rs JOURNAL_SHARE) it can swing by tens of MiB on a small filesystem. Measured by the guest check a_removed_fill_gives_back_all_but_the_reservation (GCE r947, 256 MiB): 262377472 free before a fill, 258097152 after removing it and syncing, 4280320 short, the reservation 4201408 then 7310096 (3108688 of it); the rest is other unflushed metadata. Decide how statfs should count it: set aside the reservation's ceiling as ext4 sets aside its journal (stable, but hides up to an eighth of a small filesystem, and writes would succeed past f_bfree), or report used without the journal-proportional part (df shows room a write may not get), or flush the trees on syncfs when the reservation is a large share of a small device.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 statfs's free space after a fill is removed and synced is within 1% of before, on a 256 MiB filesystem (generic/015 passes on xfstests-fxfs-guest and -dax; take it from KNOWN_FAILURES)
- [ ] #2 fill-to-ENOSPC checks and xfstests' enospc group still pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
