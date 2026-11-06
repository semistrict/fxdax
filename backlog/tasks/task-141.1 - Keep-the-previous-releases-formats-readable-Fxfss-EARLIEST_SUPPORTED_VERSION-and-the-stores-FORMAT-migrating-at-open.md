---
id: TASK-141.1
title: >-
  Fxfs refuses every version but its own: EARLIEST_SUPPORTED_VERSION is
  LATEST_VERSION, a version-60 filesystem refused at mount
status: Done
assignee:
  - '@claude'
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 01:50'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 12500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
AGENTS.md (no migrations, the user, 2026-11-05): new code refuses the old format; nothing is kept to read old data. Fxfs is at version 61 (crates/fxfs/src/serialized_types/types.rs:45; StoreInfoV61, 321fb94f2, TASK-136.3) and still reads 60: EARLIEST_SUPPORTED_VERSION is 60 (types.rs:59), so super_block.rs:641-660 refuses only what is older than that, and a version-60 store's layers are written again in 61 at open (upstream's FlushReason::UpgradeVersion; its test, volume.rs:590 test_flush_on_open_to_update_version, runs only because the two differ). allocator.rs:150-155 says EARLIEST_SUPPORTED_VERSION is LATEST_VERSION, which no longer holds.

Set EARLIEST_SUPPORTED_VERSION to LATEST_VERSION, as upstream's own rule for a breaking change says (types.rs:51-58), so that a filesystem of any other version is refused at mount, its version named; fxdax code that only reading version 60 needs goes with it. The store's FORMAT and the bucket's layout::FORMAT refuse every other format already (TASK-165.6, TASK-165.3).

Rewritten 2026-11-05 from "Keep the previous release's formats readable: Fxfs's EARLIEST_SUPPORTED_VERSION and the store's FORMAT, migrating at open", which AGENTS.md's rule forbids.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 EARLIEST_SUPPORTED_VERSION equals LATEST_VERSION, and what says so (allocator.rs:150-155, types.rs's deviation note) is true
- [x] #2 A test: a filesystem whose super-block or layers say version 60 is refused at mount, its version named, nothing of it written
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. EARLIEST_SUPPORTED_VERSION defined as LATEST_VERSION (types.rs), with the deviation note: under the no-migrations rule every format change is breaking, so the two can't drift.
2. Delete what only reading older versions needs, fxdax's: StoreInfoV52 and its migration to V61; AllocatorInfoV59 and V32 (dead since 60) and allocator_info_migrates_from_version_32; the allocator's full compaction for older layers and its test; ObjectStore::earliest_writable_version and LSMTree::get_earliest_version_above (base layers kept out of upgrades), and test_base_layers_of_older_versions_stay. flush.rs and volume.rs's open go back to upstream's get_earliest_version. Upstream's own generic old-version code (UpgradeVersion, OLD_KEY_SERIALIZATION_VERSION, AES_JOURNAL_ENCRYPTION_VERSION) stays: unreachable, and deleting it widens the upstream diff.
3. Refusals say FxfsError::InvalidVersion with the version named: the super-block's three checks; a manifest of any version but the latest; a base layer of any other version, at store open.
4. allocator.rs's Compactions comment, types.rs's header, errno.rs's InvalidVersion row comment made true.
5. Tests: super_block.rs, a mount of an image whose super-block, journal checkpoint or earliest_version (what layers of 60 make it say) is 60 refused with InvalidVersion naming 60, the device unwritten; base.rs, manifests of 60 refused, a base layer of 60 refused at open.
6. Verify: nextest -p fxfs -p fxfs-posix -p storage_device, fxfs-posix model hunt at 1000 cases, scripts/test.sh workspace on macOS, upstream-changes.py --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, the user: assume nothing uses fxdax until they say otherwise, so formats may change with no migration (AGENTS.md). On hold until then.

2026-11-05 triage: rewritten to fit AGENTS.md's no-migrations rule (75bd872ba): what it asked, the previous release's formats read and migrated at open, is what the rule forbids; what fits is its inverse, Fxfs refusing the version 60 it still reads.

2026-11-05, done in c6b60d679.

Deleted (fxdax's own reading of older versions): StoreInfoV52 and its #[migrate_to_version(StoreInfoV61)]; AllocatorInfoV59 and AllocatorInfoV32 (dead since EARLIEST_SUPPORTED_VERSION was 60) with allocator_info_migrates_from_version_32; the allocator Flusher's full compaction while a layer is older, with test_compaction_merges_layers_of_older_versions; ObjectStore::earliest_writable_version and LSMTree::get_earliest_version_above, base layers' exemption from upgrades, so flush.rs's UpgradeVersion early exit and volume.rs's open go back to upstream's tree().get_earliest_version(); test_base_layers_of_older_versions_stay became test_base_layers_of_older_versions_refused. Kept: upstream's generic upgrade code (FlushReason::UpgradeVersion, OLD_KEY_SERIALIZATION_VERSION and AES_JOURNAL_ENCRYPTION_VERSION readers, volume.rs's test_flush_on_open_to_update_version, which skips itself now), unreachable; decision in OPEN_QUESTIONS.md.

EARLIEST_SUPPORTED_VERSION is defined as LATEST_VERSION, so no bump can part them. Refusals are FxfsError::InvalidVersion with the version named: the super-block's three checks (were plain anyhow messages: EIO in the guest; now EINVAL, logged, by fxfs_posix::errno, which fxfs-linux's fxfs_open returns as is); SuperBlockManager::load, both instances failing, keeps the first's error and kind, where it made a new message; a manifest of any version but the latest (new_volume_over_base); a base layer of another version at ObjectStore::open, as the super-block's earliest_version covers only the filesystem's own layers. fxfs-linux and fxfs-posix have no version handling of their own: only the errno row, whose comment now says older or later. Host: fxdax::Error::errno maps no Fxfs InvalidVersion (only extent_store::Format), unchanged.

Tests: super_block.rs test_mount_of_the_previous_version_is_refused (super-block's version, journal checkpoint's, earliest_version = what layers of 60 make the journal record): InvalidVersion, message naming Version { major: 60, minor: 0 }, no device write or free, root unchanged; base.rs test_base_layers_of_older_versions_refused, test_base_manifests_refused (now also an earlier manifest version). All five red with EARLIEST_SUPPORTED_VERSION set back to 60 (mounted / opened). A layer test through a real store was tried: only stores needing a flush report versions to the journal, and that flush's compaction rewrites the empty older layer, so the earliest_version case writes the header.

Runs: cargo nextest run -p fxfs -p fxfs-posix -p storage_device 890/890 (2 skipped); FXDAX_MODEL_CASES=1000 whatever_a_vfs_asks_reads_back_as_a_model_says passed (569 s; 1961 power cuts, 3518 replays); FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast on macOS 2119/2119; scripts/test-linux.sh (Lima) workspace 2300/2304, the 4 failing in fxdax and failing on origin/main 752515020 too in the same Lima (5 there): builds_are_stamped_with_their_commit (Lima's copy has no git), copy_on_write_windows_run_under_firecrackers_seccomp_filters (worktree's vendor/firecracker checked out at a2db2b3, main pins e64c4b283), a_store_after_a_workers_write_waits_for_one_lift, windows_larger_than_the_map_count_read_whole (SIGKILL); scripts/upstream-changes.py --check passes. No GCE run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fxfs refuses every version but its own: EARLIEST_SUPPORTED_VERSION is LATEST_VERSION, refusals are FxfsError::InvalidVersion naming the version (super-block, manifest, base layers), and fxdax's code for reading older versions is gone (StoreInfoV52's migration, AllocatorInfoV59/V32, older-layer compactions, base layers' upgrade exemption). Verified by new tests red with 60 back, nextest of fxfs/fxfs-posix/storage_device, the model hunt at 1000 cases, the macOS workspace suite, and Lima's, whose 4 failures main shares.
<!-- SECTION:FINAL_SUMMARY:END -->
