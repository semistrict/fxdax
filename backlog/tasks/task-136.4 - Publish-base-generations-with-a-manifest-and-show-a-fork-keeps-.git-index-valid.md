---
id: TASK-136.4
title: >-
  Publish base generations with a manifest, and show a fork keeps .git/index
  valid
status: Done
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-11-01 02:00'
labels: []
dependencies:
  - TASK-136.3
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 471000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Infrastructure keeps a base with a full checkout current. Attach needs a generation that stays put and a small description of it, so that no guest replays a base's journal. A fork is also only worth having if its first `git status` trusts the index: object IDs and stored times should carry over, but uid and racily clean entries can force a rehash.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A publisher updates a repo in a base store and publishes a generation and its manifest, with earlier generations still attachable
- [x] #2 A test shows the first `git status` in a fork lstats every tracked file and hashes none, as the sandbox user
- [x] #3 What a publisher must do for that to hold (uid, index refresh after racy timestamps) is written in doc-10
- [x] #4 A generation is a fork alone, with no `snapshot_volume`, and its volume is flushed first, so that its manifest leaves nothing in the journal
- [x] #5 Each generation holds every object of the earlier ones, unreachable ones included
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fxfs: ObjectStore::manifest refuses while the journal holds anything of the store.
2. Guest: FXFS_IOC_FLUSH with the volume frozen, FXFS_IOC_MANIFEST with the filesystem frozen, FXFS_IOC_IMPORT. A live fork needs the whole device quiescent, so the manifest comes at that point and no compaction hold is needed.
3. fxfs-guest-shared-volumes(-dax): a publisher VM with Git, two generations, a consumer VM attaching them as it runs, strace of the first git status, a reboot.
4. doc-10: the order of the steps, and what Git needs of a publisher.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Ramon decided on 2026-10-31 that the feature is generic shared volumes and that a sandbox keeps a volume current, not a publisher on the host: doc-10, Publishing. This task therefore also needs the guest's part: a way to publish a mounted volume (RootVolume::publish_volume, with no compaction before the fork) and to make a volume over a base from a manifest (new_volume_over_base). Guests can do neither yet, so no guest has mounted a volume over a base.

Design change from the earlier note: no publish_volume in the guest and no hold on compaction. The host's fork refuses with anything open on the device, so the filesystem has to be frozen anyway, and a manifest taken then can't be invalidated by compaction. The flush has to come before that freeze, with the volume frozen alone, so some other volume stays mounted. Git findings: the index names a file's owner, not who runs Git, so a chown after the index is written makes every file hash (permanent check in the test). With settle_index left out, a one-off mutation run opened exactly the four files the last update wrote, d0..d3/f000, and no others.

Validation: scripts/test.sh passes on macOS (1877 workspace tests) and Linux (2016, with a real FXDAX_BUILD_COMMIT), upstream-changes --check passes, check-guest passes on arm64. fxfs-guest-shared-volumes and -dax pass. Full guest suite on arm64: all OK but for these. fxfs-guest, -dax, -format and pjdfstest-fxfs-guest-dax failed for how I ran them (no commit stamp in the Lima tree, a full 7.8 GB /tmp) and pass rerun with those fixed. fxfs-guest-checks-dax, -delay and -small hit the harness's 180 s limit, but their guests printed OK and shut down at 264, 331 and 438 s. fxfs-guest-snapshot-volumes-dax fails about every second round with EBUSY from the host's resize, straight after killing the VM, with this work (round 2 of 3) and on clean 51518829a (round 2 of 8): holder_or_hold refuses a holder that is going. Not fixed here: out of scope. windows_larger_than_the_map_count_read_whole was OOM killed once in Lima and passed in the next full run. Not run: x86, dax-gate.sh, a Linux-sized checkout (TASK-136.1).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Guests now publish and import shared volumes. FXFS_IOC_FLUSH with the volume frozen, FXFS_IOC_MANIFEST with the whole filesystem frozen, the host forks, and FXFS_IOC_IMPORT makes a volume in another sandbox; ObjectStore::manifest refuses while the journal holds anything of the volume. Verified by fxfs-guest-shared-volumes(-dax): a publisher VM with real Git publishes two generations, a consumer on its own store attaches both as it runs, newest first, and strace shows its first git status lstats all 200 tracked files and opens none, while files chowned afterwards are all opened. The unreachable commit survives, private writes survive a remount and a reboot. A mutation run showed the index rule is needed. doc-10 has the steps and Git's rules.
<!-- SECTION:FINAL_SUMMARY:END -->
