---
id: TASK-160.24
title: 'Fxfs: fxdax-only code in child modules of upstream files'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 15:40'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 595000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
data_object_handle.rs was +2,596/-141 against upstream at the review, 1,637 lines of it pure additions (write_many, extents_in, txn_map_write, clones, zero_in_commits); volume.rs +1,197, 856 pure (snapshots, bases). Proposal: fxdax's additions in child modules (data_object_handle/{dax,write_many,reflink}.rs and volume/{snapshot,base}.rs), impl blocks, so that the upstream diff of those files shrinks. After TASK-153.5 and 153.12.

At 9f81f6018, writable DAX's deletion (37c331a6c) took txn_map_write and DaxWrite out of data_object_handle.rs (889 lines removed, 312 added), so the figures above are high by about that. Its additions now: write_many and write_many_reserved (crates/fxfs/src/object_store/data_object_handle.rs:1171, 1180; DAX writeback's path through fxfs_write_many), extents_in (2115, what the window maps from), clone_range_in_commits (2298) and zero_in_commits (2354). A dax.rs child would hold extents_in alone; write_many.rs and reflink.rs the rest. Measure the diff again with scripts/upstream-changes.py --diff before moving.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Moved, behaviour unchanged
- [ ] #2 upstream-changes --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure the fork's diff against upstream (git diff --no-index --numstat of crates/fxfs/src against src/storage/fxfs/src at the recorded Fuchsia commit; upstream-changes.py --diff shows upstream's own changes, not the fork's).
2. data_object_handle.rs: fxdax's additions into child modules, impl blocks of their own, each with its tests: dax.rs (ExtentAccess, extents_in), write_many.rs (write_many*, FAIL_COMMITS), reflink.rs (txn_clone_range), in_commits.rs (clone_range_in_commits, zero_in_commits, end_of_room, cost_of_freeing), tail.rs (zeroing past the end, upstream's grow inline; Fuchsia's header, cited), write_once.rs (InPlace, in_place, reallocate_range), snapshot.rs (FrozenPage, frozen_pages), testing.rs (the counting helpers the tests share). Upstream's four test helpers pub(super).
3. volume.rs: snapshot.rs (snapshot_volume), base.rs (publish_volume, new_volume_over_base), lazy.rs (lazy volumes' tests), testing.rs (shared helpers); do_fsck pub(super).
4. The mutant patches that named moved code regenerated against their new files (mutants.sh --check).
5. Moves in commits of their own; check that every removed line lands in a child and that the test set is unchanged; fxfs, fxfs-posix, storage_device suites, the model hunt at 1000, the macOS workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on task-160-24 (ee95b0f71, 59f9fed29), moves only, each a commit of its own.
Measured: git diff --no-index --numstat of crates/fxfs/src against src/storage/fxfs/src at the recorded Fuchsia commit 90e54e09 (upstream-changes.py --diff shows upstream's own changes between revisions, not the fork's, so it can't measure this). data_object_handle.rs +2276/-192 to +263/-178; volume.rs +1203/-146 to +47/-157; every file Fxfs shares with upstream +14842/-6579 (21421) to +11673/-6576 (18249).
Moved: data_object_handle/{dax.rs (ExtentAccess, extents_in), write_many.rs (write_many, write_many_reserved, FAIL_COMMITS and their helpers), reflink.rs (txn_clone_range), in_commits.rs (clone_range_in_commits, zero_in_commits, end_of_room, cost_of_freeing), tail.rs (txn_zero_tail, txn_grow_size, zero_tail_before_growth, zero_tail; upstream's grow's zeroing, so Fuchsia's header and citation), write_once.rs (InPlace, in_place, reallocate_range), snapshot.rs (FrozenPage, frozen_pages), testing.rs (JournalUsages, Commits, counted_filesystem and the helpers write_many's and in_commits' tests share)}; volume/{snapshot.rs (snapshot_volume), base.rs (publish_volume, new_volume_over_base), lazy.rs (lazy volumes' tests), testing.rs}. Each child holds its code's tests. Hooks left upstream: the mod lines and re-exports (ExtentAccess, FrozenPage, FAIL_COMMITS at their old paths, which the guest names), and pub(super) on five upstream test helpers (test_filesystem, create_object_with_key, test_filesystem_and_object_with_key, TEST_DEVICE_BLOCK_SIZE, do_fsck). Left in place: preallocate_range_in_extents_of, which is upstream's preallocate_range's body with a parameter, and fxdax's edits inside upstream functions.
Behaviour unchanged: every line removed from the two files is in a child (modulo pub(super), imports and rustfmt's reflow); the crate's tests are the same 774 test functions and 26 test_case lines before and after. The mutants clone-counts-holes and punch-by-range named the moved lines: regenerated against in_commits.rs; crates/fxfs-posix/mutants.sh --check 17 of 17, guest-mutants-gce.py --check 6 of 6.
Runs (macOS): cargo nextest run -p fxfs -p fxfs-posix -p storage_device 887 of 888, the last (the model test) cancelled by a SIGTERM from outside (the harness ending another background run), then passed alone in the hunt; FXDAX_MODEL_CASES=1000 model hunt passed (1119 s); FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast 2121 of 2121; upstream-changes.py --check passes; fxfs-linux type-checks for aarch64-unknown-none-softfloat in Lima (check-guest.sh's cargo check, its own sysroot and target dir). Not run: GCE (Linux workspace, guest cases).
Other files still far from upstream, for a follow-up if wanted: allocator.rs +2744/-1201, journal.rs +1911/-313, persistent_layer.rs +823/-63, filesystem.rs +703/-588, object_store.rs +632/-351, super_block.rs +537/-83, reader.rs +537/-34.
<!-- SECTION:NOTES:END -->
