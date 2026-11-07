---
id: TASK-165.7
title: >-
  Forks, points and deletion only in the bucket: no local lineage, layers,
  ledger, branch records, handles or claims
status: Done
assignee: []
created_date: '2026-11-05 03:16'
updated_date: '2026-11-07 21:29'
labels: []
dependencies:
  - TASK-165.3
  - TASK-165.5
  - TASK-165.6
  - TASK-164
  - TASK-160.8
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 629000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 7. The local lineage and remote.rs's claim, marker and take_parent machinery keeping it agreeing with the bucket are where TASK-79.5, 79.20, 79.71, 79.72, 79.77, 79.78, 79.82, 79.85 and 79.95 were. A fork becomes the parent's commit naming the child and the child's record; a child is a store opened from the bucket. Starts after TASK-164 and TASK-160.8 (extents.rs and extentd/stores.rs change in all three); lands in one batch with TASK-165.8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fork, a fork of a point, a point and a deletion go through extent_remote::branch and the parent's commit alone; the child's directory is made once its record exists, and opening it recovers the child's state from the bucket and takes it as its own, O(extents) of the parent at the fork, rather than sharing the parent's state in memory: a child tied to nothing of its parent's but the bucket opens anywhere, attached or not, and sharing in memory needed the index TASK-165.9 removed (decided 2026-11-05, doc-14 Decisions)
- [x] #2 extent_store's lineage.rs, layers.rs, ledger.rs, branch_records.rs and host.rs's handles are gone, and remote.rs's Claim, origin-* files, take_parent and unfork
- [x] #3 remote.rs's fork tests, fork_faults.rs, restore_faults.rs and the extentd fork and restore tests pass rewritten as doc-14's table says; each removed test is listed in the notes with what replaced it
- [x] #4 A fork of a 10 GiB base on GCE is timed before and after, and recorded in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
From TASK-165.6: an image's own file bucket is <image>/bucket, inside its directory, which its clones' handles name. That holds only while the local lineage keeps an image's directory as long as its clones live; once forks are only in the bucket, a deleted image's directory goes with its bucket under its clones. Move it (beside the image, or a bucket the caller names) in this step.

Implemented on branch task-165-7 (rebased on origin/land-l, i.e. main + TASK-165.6), three commits with 165.8's step:
- 875bf7fb2 extent_store: Data::Remote(Location), local format 7 (refactor; no behaviour change).
- eaeffc8b4 fxdax: forks, points and deletion only in the bucket (host_store::fork/make_child, remote::take_branch, Remote::unfork, child_free; claims, child markers, take_parent, handles gone; image's own bucket beside it, <image>.bucket).
- 5dfaeb0e5 extent_store: a store holds one branch (lineage.rs, branch_records.rs, layers.rs, ledger.rs deleted; Dir::claim refuses a second opening in a process; delete_host removes the directory whole).
Decisions: OPEN_QUESTIONS.md, "Forks, points and deletion are the bucket's alone".
TASK-184: done on main meanwhile (write_new); kept for markers and fences, its claim cases gone with claims. TASK-186: moot, closed.

Tests deleted (fxdax, step 1; replaced by the bucket's fork protocol tests in fork_faults/restore_faults and those listed as added): claims_read_as_written, deleting_a_fork_removes_its_placement, deleting_a_handle_never_described_removes_it, deleting_an_emptied_handle_finishes_it, each_write_of_a_childs_upload_meeting_the_collector, forks_are_named_by_their_branch, forks_failed_whose_deletion_failed_keep_their_handle_and_branch, forks_failed_whose_handles_are_deleted_later_leave_branches_that_took_their_childs_id_be, forks_failed_whose_host_crashed_as_they_were_deleted_are_deleted_again, forks_left_by_a_crash_take_no_branch_that_took_their_childs_id, forks_taking_records_of_their_own_take_none_of_another_branch, forks_their_parents_branch_names_not_take_records_of_their_own, forks_to_a_branch_the_store_has_make_nothing, forks_whose_host_crashed_before_their_record_are_deleted_in_object_storage_too, forks_whose_parents_are_open_nowhere_take_their_parents_branch_and_records_of_their_own, forks_whose_parents_were_deleted_here_take_their_parents_branch_and_records_of_their_own, forks_whose_record_was_lost_record_themselves, forks_whose_segment_may_land_yet_take_no_branch_of_their_own, forks_whose_segment_never_lands_take_records_of_their_own_once_their_parents_are_taken, forks_with_records_of_their_own_are_deleted_with_their_handles, restores_fork_anew_over_forks_that_failed, restores_over_what_a_dead_host_left_fail_until_it_is_deleted, stores_are_cleaned_once_the_forks_sharing_their_segments_go, what_a_child_uploads_of_what_it_inherited_outlives_the_collector.
Tests added (fxdax, step 1): deleting_a_store_never_described_removes_it, deleting_an_emptied_store_finishes_it, forks_failed_whose_deletion_is_refused_leave_their_branch_to_its_placement, forks_to_a_child_forked_before_make_nothing, forks_whose_host_crashed_stand_once_their_segment_lands, restores_fork_anew_over_forks_left_unmade, restores_over_what_a_failed_cleanup_left_fork_anew, stores_with_no_uploader_fork_nothing; plus forks_orphaned_by_a_lost_host_keep_what_they_inherited_until_deleted (fork_faults). Rewritten for the bucket: fork_faults/restore_faults step lists (FORK_STEPS, CLEANUP_STEPS), extentd fork/restore tests, storage clone tests, fxdaxctl fork tests.
Tests deleted (step 2): extent_store's 79 tests of forks, lineages, layers, ledger, handles, inherited segments and shared files (tests/lineage.rs, tests/shuttle/lineage.rs and their cases in store.rs/host.rs/shuttle), e.g. forks_share_their_parents_state, files_go_once_every_holder_let_them_go_durably, needs_forks_to_wait_for_framings, needs_openings_and_deletions_to_wait; fxdax deletions_stopped_at_a_branchs_records_finish, unmarking_removes_new_files_a_crash_left (unmark gone: a store's markers are in its own directory, removed with it). Added: extent_store stores_open_once, stores_name_their_branch_and_bucket, deleting_stores, deletions_delete_their_branch_elsewhere_first, deletions_remove_the_stores_directory_durably, deletions_stopped_at_any_step_finish, checkpoints_replace_the_index_files_before. Shuttle needs_closes_to_wait_for_framings_after_a_failed_flush writes on after reopening, which finds the race again.
Runs: macOS extent_store+extent_remote+fxdax 671/671 (HEAD of step 2), step 1 alone 729/729; Linux (OrbStack, rust:1) step 2 847/853 and step 1 905/911, failing only eu-stack (3), grants ENOENT (3).

Runs at 42e484d75 (with TASK-165.8): see TASK-165.8's notes. GCE suites still to run before landing: forks, snapshots, volumes, migration, live fork.

Rebased onto main 752515020 (land-l landed): 8fa81ef66 (Data::Remote), bb8c0f4e2 and 42db93a24 (TASK-165.7), b447c610b (TASK-165.8); hashes above are before the rebase. At b447c610b: macOS FXDAX_TEST_PARTS="workspace shuttle sim" scripts/test.sh --no-fail-fast green (workspace 2027/2027, sim 47/47, shuttle 38+6+6+32+10+18); Linux extent_store+extent_remote+fxdax 834/840, failing only eu-stack (3) and grants ENOENT (3).

GCE r701 (68b5f2a02): fxfs-guest-import failed, ENOTSUP: it forks a base imported offline, whose ExtentStore had no uploader. Fixed in ced9a66bb: ExtentStore::kept_in_bucket attaches an uploader to the store's described bucket before a fork or point where none is attached (followers excepted); host_store::fork still refuses a store with no uploader (forks_with_no_uploader_make_nothing). New tests: stores_made_of_images_fork_where_no_daemon_is (red with r701's error, then green), images_imported_where_no_daemon_is_fork_there (fxdaxctl import then fork, file bucket), stores_with_no_uploader_attach_one_to_fork (replaces stores_with_no_uploader_fork_nothing). Offline forkers checked: fxdaxctl fork/point (attach already), Storage::clone_to (images attached), daemon stores (attached at opening). Found meanwhile: a memory:// bucket loses what it acknowledged once its process exits (README and OPEN_QUESTIONS say so). Runs at ced9a66bb: macOS extent_store+extent_remote+fxdax 659/659; Linux 837/843, failing only eu-stack (3), grants ENOENT (3). r701's import case to rerun on GCE.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Forks, points and deletion are the bucket's alone; a child's store is made, and takes its state, once the bucket has its record (TASK-165.7, 2026-11-05)**

A store's directory holds its branch alone. A fork is the parent's commit naming the child (`Store::commit_fork`), whose acknowledgement gives the child its record; a fork of a point, `branch::fork_point`; only then is the child's directory made, a store of its own (`host_store::fork`). Decided where doc-14 left it open:
- **The child's store takes the child's state at the fork** (`remote::take_branch`: every extent adopted at its location in the bucket, the capacity, the spent IDs, the root, the marker naming the fork's commit), so that it opens anywhere, attached or not, and reads what it inherited from the bucket or the host's cache. A fork costs a recovery (the checkpoint the writer just wrote) and a commit of every extent's place: O(extents), where the local lineage's was constant. Sharing the parent's state in memory (AC #1's last clause) needs the index gone, which is TASK-165.9's. *Instead:* an empty child that takes its state when first attached, which an unattached open would find empty. *Undoing it:* `make_child`'s `take_branch`.
- **What a fork whose host dies, or whose cleanup fails, leaves in the bucket stands**: an orphan, a live branch no store holds, as doc-14's "the fork stands" has it. `fxdax-gc --branches` lists it and `--delete-branch` deletes it (`forks_orphaned_by_a_lost_host_keep_what_they_inherited_until_deleted`); fork_faults and restore_faults check that only the fork's child is ever one, and only of a fork that failed. Before, the fork's handle named the child, and deleting the handle finished it (TASK-79.78). *Instead:* a file in the fork's directory naming the child and its owner until the fork is made or deleted: a claim, which doc-14 removed. *Undoing it:* that file, written before the fork's commit, which `host_store::delete` would read.
- **A fork needs its store's uploader**: a store with none forks nothing (`ENOTSUP`).
- **A failed fork is deleted in the bucket by what its uploader knows**: the origin of the fork's segment, once the uploader took it (`Shared::forks`), else as a branch of its own (`Kind::Own`), so that a segment landing late finds it deleted. In memory alone: a host crashing loses it, and the fork stands.
- **What fxdax keeps beside a store goes into its fork's directory at the fork** (`extents::inherit`): what is attached to it, the branches marked public, and `ancestry`, the branch and directory of each store it was forked from, nearest first, which `ExtentStore::ancestry` and the boot profile (in the base's directory, as before) read. A store made public after its forks were made is not public in them; before, the directory they shared made it so for every fork opened after. *Instead:* each fork reading its parents' directories, which outlive nothing now.
- **An image's own file bucket is beside its directory**, `<image>.bucket`, which its clones' descriptions name, and which nothing removes: it holds the clones' branches once the image is deleted. The in-process collector runs for a `file:` bucket whose path ends `.bucket`. *Undoing it:* `OWN_BUCKET`.
- **fxdaxctl's local commands keep a store's branch in the bucket its description names**, not `FXDAX_EXTENTD_REMOTE`'s, which they still require, with the token, to change an existing store.
- **A directory with no description is no store** (`host_store::unmade`): what a store's making, or a fork's, left. Deleting it removes it; a restore over it removes it first. It replaces the failed fork's handle (TASK-79.84).

TASK-186 (claim_origin's unordered read-then-write) is moot: no claim file is written. TASK-184's fix (each durable write beside a store a file of its own, `write_new`) stays for markers and fences.

Closing, 2026-11-07 (main d9b29dbd9; landed with TASK-165.8 and 165.9, c9207e5d4):
- AC #1 rewritten to the decision made (doc-14, Decisions; this task's 2026-11-05 decision): a child takes its state from the bucket as its store opens, O(extents), not sharing the parent's in memory. On main: host_store::fork (the parent's commit naming the child, Remote::forked waiting for its acknowledgement and record, then make_child) and a point's fork through branch::fork_point; the child's store recovers as any store opens (TASK-165.9, Store::recover); deletion through branch::delete. Tests: forks_whose_host_crashed_stand_once_their_segment_lands (remote.rs), forks_orphaned_by_a_lost_host_keep_what_they_inherited_until_deleted (fork_faults.rs), stores_open_once and deleting_stores (extent_store tests/host.rs).
- AC #2: extent_store's lineage.rs, layers.rs, ledger.rs and branch_records.rs are gone, and host.rs holds no handle (a directory holds one branch); remote.rs has no Claim, origin-* file, take_parent or unfork (the fork's deletion in the bucket, for a fork that failed after its commit, is extent_store::upload's Remote::unfork, branch::delete_fork). remote.rs's code went from 1,687 lines to 178 (TASK-165's notes).
- AC #3: the rewritten tests and those removed, each with what replaced it, are listed above (Tests deleted, Tests added, Tests rewritten); fork_faults.rs and restore_faults.rs pass on main (macOS workspace 2103/2103; CI 42401937262).
- AC #4: GCE r970 (scripts/fork-time-gce.sh, land-x4), docs/measurements/fork-of-a-10-gib-base-2026-11-07.md: a 10 GiB base (2,560 random 4 MiB files) imported through the daemon, forked five times on main before the restructure (aee7cad93) and after (3bdec3934), on one host: a fork 27-29 ms after against 33-35 ms (the first 43 ms against 142 ms); the fork's first open 29-31 ms against 11-12 ms, its state recovered from the bucket, O(extents); fork and first open 56-60 ms against 45-47 ms. Found by it: an import inside extentd keeps nothing in the daemon's part cache (Storage's uploader has none, host_store::keeping) and its check of the filesystem it made reads the image back from the bucket: 6.8 GB downloaded for the 11 GB uploaded, the import 221 s against 119 + 54 s pre-restructure; reported to the user, not filed yet.
DoD: the macOS workspace and CI 42401937262 on main; the guest suites on GCE (r910-r914, r970); scripts/upstream-changes.py --check passes on main; no upstream file changed.

The import finding above is filed as TASK-237 (Medium), 2026-11-07.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Forks, points and deletion are the bucket's alone: a fork is the parent's commit naming the child, the child's store made once its record exists, recovering its state from the bucket as it opens (O(extents), the decision AC #1 now states); local lineage, layers, ledger, branch records, handles and claims are gone. A fork of a 10 GiB base on GCE (r970): 27-29 ms against 33-35 ms before the restructure, its first open 29-31 ms against 11-12 ms (docs/measurements/fork-of-a-10-gib-base-2026-11-07.md). Verified by the tests named in the notes, CI 42401937262 and the macOS workspace on main.
<!-- SECTION:FINAL_SUMMARY:END -->
