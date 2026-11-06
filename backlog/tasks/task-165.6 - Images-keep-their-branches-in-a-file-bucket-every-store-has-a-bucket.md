---
id: TASK-165.6
title: 'Images keep their branches in a file:// bucket: every store has a bucket'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-06 02:21'
labels: []
dependencies:
  - TASK-165.1
documentation:
  - doc-14
parent_task_id: TASK-165
priority: medium
ordinal: 628000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 6. Storage's images (the fxdax CLI's, mounted by Storage::open, also of stores extentd made) are the one kind of store without a bucket, keeping local forks (clone_to) and local cleaning alive. Once they keep their branches in a file bucket, nothing needs the local lineage.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A store's description names its bucket; Storage::create makes an image whose branch is in a file bucket, and clone_to forks it there
- [x] #2 Storage runs the collector in-process after commits, as it cleans now; tests/storage.rs and the filesystem tests pass
- [x] #3 No code opens a store without a bucket (host_store, storage)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: a host store's description (StoreFile) names its bucket's URL; create_host requires one; Lineage::bucket, host_bucket.
2. fxdax host_store: one registry of buckets by URL (extentd's own registered under its URL); create names this host's bucket (FXDAX_EXTENTD_REMOTE) or, on a machine that is no host, a file bucket of the store's own in its directory; open resolves the description's bucket and refuses a store naming none (made by older code); the token of copies never placed: the host's, or 1.
3. Refactor: ExtentStore::fork_durable's fork protocol (claim, fork_host_with, forked, failed forks deleted) moves to host_store::fork, shared with Storage.
4. Storage: an image's device holds its store kept in its bucket (an uploader attached, Remote); a commit fails once uploading failed, waits on the lag bound, cleans locally, and nudges the file bucket's collector; closing waits for uploads; clone_to forks through host_store::fork, so the clone is a fork in the bucket; delete deletes the branch in the description's bucket.
5. storage/collect.rs: one collector per file bucket per process, a pass on a thread of its own after a commit, at most one a minute.
6. extentd: attaches a store to the bucket its description names (an image's file bucket, or the daemon's); deletes there.
7. Tests for each; decision on older images in OPEN_QUESTIONS.md; macOS workspace+shuttle, Linux Docker, GCE guest cases with images.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (commits 32d5654bb, 8550a3de1):
- extent_store: local format 6. A host store's description (store) names its bucket's URL; create_host requires one; Lineage::bucket, host_bucket. Stores of memory (Store::create) name none. Format 5 and older are refused, opened or deleted, by extent_store::Format, which fxdax's Error::errno maps to ENOTSUP (AGENTS.md: no migrations). doc-11 updated.
- fxdax host_store: one bucket per URL per process (host_store::bucket; extentd's own registered under its URL, use_bucket in tests). create names this host's bucket (FXDAX_EXTENTD_REMOTE) or, where none is, <store>/bucket, a file bucket of its own which its forks share. open returns the store with its bucket (Opened): no store opens without one. token(): FXDAX_EXTENTD_TOKEN's, or 1.
- Refactor first (32d5654bb): ExtentStore::fork_durable's fork protocol is host_store::fork, shared with images.
- Storage (storage/kept.rs): an image's store with an uploader (Remote) attached. After a commit: fails once uploading failed (Remote::failed, new), waits on the lag bound, cleans locally as before, starts the collector. Closing waits for uploads. clone_to forks through host_store::fork (a fork in the bucket); Storage::delete deletes the branch in the description's bucket.
- storage/collect.rs: the collector of an image's own bucket, one per bucket per process, a pass on a thread of its own after a commit, at most one a minute, grace 60 s. A host's bucket is never collected in-process.
- extentd attaches a store to the bucket its description names (an image it serves keeps its branch in the image's file bucket) and deletes there.
- scripts/test-e2e.sh: the shared daemon keeps stores in a file bucket on /dev/shm, not its memory: the cases open its stores in their own processes, where a memory bucket is another (a restore's claim of a point's fork found no point there).
- Decisions: OPEN_QUESTIONS.md, 'Every store names its bucket ...'.

Tests, each red with its behaviour removed (mutated and run): storage::kept images_keep_their_branches_in_a_file_bucket_of_their_own (close without waiting for uploads), clones_are_forks_in_their_images_bucket (fork without the remote), deleting_images_deletes_their_branches (delete without the bucket), commits_start_the_collector (no after_commit), the_collector_reclaims_what_images_freed, images_in_a_hosts_bucket_are_not_collected_here (keeps_own_bucket always true); host_store older_stores_are_refused_by_their_format (format 5 accepted), stores_name_their_hosts_bucket_or_one_of_their_own (host's URL ignored); extent_store stores_and_their_forks_name_their_bucket (bucket line not written), layout descriptions; extentd keeps_branches_in_the_buckets_their_stores_name (attached to the daemon's bucket, Linux).

Runs: macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh: green (workspace 2129/2129). Linux in Docker, cargo nextest -p fxdax -p extent_store: 694/701, the 7 environment-only (eu-stack x3, grants ENOENT x3, build stamp). GCE r620, the 22 guest cases using images: 21 pass; fxfs-guest-crash-stray-dax fails, as it does on main at 75bd872ba (r623, round 2: exited 0, not 159), also r621/r622 here: filed as TASK-187, not this change's.

Overlap: TASK-165.5 (remote.rs: Remote::failed added, block_on pub(crate), 3 test lines take Opened's store). Collector work (TASK-173/174/183): Collector's public API only. TASK-165.7 must move an image's bucket out of <image>/bucket once the local lineage no longer keeps the image's directory as long as its clones.

CI 42119298585 (land-l, rebased on ef8223900): control imports_images and imports_that_fail_leave_no_store failed with EIO on Linux: extentd's import opens the store an image made right after the image drops, and found its branch open ('branch 0x... is open'). Cause, this task's: the uploader's task said it stopped before it let go of its Arc of the store, so Remote::drop could return with the store open; images had no uploader before. Blind spot: nothing opened a store again in the same process the moment its uploader stopped, and the window is a few instructions wide, so only a slower host hit it. Fix 332dac890: the task drops its store before saying so; images_dropped_let_go_of_their_stores holds the task past its signal (failpoint remote::stopped), red without the fix with CI's error, green with. Looked around: fxdax's other threads signalling completion let go of what they hold first (FnOnce consumed) or hold nothing a reopen needs. Not Linux- or user-specific: a timing race. Runs: Linux in Docker non-root, TMPDIR=/mnt/tmp: both import tests and the image tests pass (15 extentd window tests need /dev/userfaultfd, which the container doesn't give a non-root user); Linux root 696/703 (eu-stack x3, grants x3, build stamp); macOS fxdax and extent_store 521/521.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: storage::kept's images_keep_their_branches_in_a_file_bucket_of_their_own and clones_are_forks_in_their_images_bucket, extent_store::host's stores_and_their_forks_name_their_bucket and host_store's stores_name_their_hosts_bucket_or_one_of_their_own pass in r691. AC #2: commits_start_the_collector, the_collector_reclaims_what_images_freed, images_in_a_hosts_bucket_are_not_collected_here, tests/storage.rs (fxdax::storage) and fxdax::filesystem pass in r691; r690's guest cases, images among them, 15 of 15, fxfs-guest-crash-stray-dax now passing (TASK-187). AC #3: rg on main: every ExtentStore::create/open in fxdax's non-test code goes through host_store::create/open (extents.rs), which returns Opened with its bucket, open_host refusing a store naming none (older_stores_are_refused_by_their_format passes in r691); control's imports_images and imports_that_fail_leave_no_store pass in r691.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every store names its bucket: images keep their branches in a file bucket of their own, clones fork there, Storage runs that bucket's collector in-process; no store opens without a bucket. On main: 779984c84, 05fd8f69f, 91144850c (the uploader letting go of its store, CI 42119298585). Verified: its tests and tests/storage.rs in GCE r691 (workspace 2302 of 2303, the failure TASK-190's), GCE r690 15 of 15, CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
