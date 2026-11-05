---
id: TASK-165.1
title: >-
  A local-filesystem bucket: file:// with conditional updates, and one
  conformance suite for every bucket
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 03:15'
updated_date: '2026-11-05 06:57'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 623000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 1. Development, tests and images need a bucket on the host's filesystem; object_store's LocalFileSystem refuses PutMode::Update, so a `file://` URL parses but every lease and record update retries forever and `probe` fails. Additive: nothing changes for memory://, s3://, gs://, az://. The conformance suite also pins down what every bucket must do, which today only the simulation and RustFS cases imply.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 `Store::from_url("file:///dir/prefix")` gives a bucket that passes `store::probe`: a create where an object is fails, an update of another version fails, an update of the object's version succeeds
- [x] #2 One conformance suite (create-only, conditional update and delete, ranged and suffix reads, sorted listing by prefix, missing objects) runs against memory://, file:// and the simulation's Direct, and against RustFS and Azurite when their URLs are set
- [x] #3 Two processes updating one object of a file:// bucket from the same version: exactly one succeeds (a test with two processes or two buckets on one directory)
- [x] #4 `fxdax-gc file:///...` collects a file:// bucket (tests/gc.rs)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote::local: an object_store ObjectStore over LocalFileSystem (fsync on) adding conditional updates and serialised deletes and overwrites, under a bucket-wide lock (a mutex in the process, flock of .fxdax-lock across processes), the lock file kept out of listings.
2. store::object_store_at maps file:///dir to it (made if missing), the throttle as for any store.
3. bucket::tests::conformance: what every bucket must do; run over memory://, file://, the sim's Direct, and in over_a_real_store (RustFS, Azurite).
4. Concurrent updates from one version through two buckets on one directory: one wins.
5. tests/gc.rs over file://. Docs: doc-7 decision 9, README/dev docs naming file://.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented extent_remote::local (139353b98): object_store's LocalFileSystem (fsync on) with conditional updates, and overwrites, deletes, copies and renames, under an exclusive flock of .fxdax-lock in the bucket's directory; each holding opens the file anew, so it excludes other processes and other holdings in its own; creates stay LocalFileSystem's hard links, unlocked. store::object_store_at maps file:///DIR to it (made if missing). bucket::conformance is the suite every bucket runs.

Verification:
- AC #1: store::tests::file_buckets_honour_conditional_writes: probe fails on object_store's own LocalFileSystem ("an update of another version gave Err(Unavailable)") and passes on file://. Also branches_on_a_file_store (the whole branch lifecycle: writer, fork, takeover, checkpoint, collector).
- AC #2: memory_buckets_conform, file_buckets_conform, simulated_buckets_conform; store::tests::rustfs (rustfs/rustfs Docker image, no local RustFS binary) and store::tests::azurite (Azurite in Docker) both pass, each running the suite before the lifecycle. scripts/test-azurite.sh itself exits 52 under OrbStack: its readiness curl uses --retry-connrefused, and OrbStack's forwarded port gives an empty reply until Azurite is up; its steps were run by hand with --retry-all-errors.
- AC #3: local::tests::one_update_of_a_version_wins: 32 concurrent updates from one version through two buckets on one directory, 8 workers: exactly 1 wins, 31 get Precondition, the object holds the winner's bytes. With the lock removed from update it failed 3 of 3 runs.
- AC #4: tests/gc.rs collects_a_file_bucket: fxdax-gc, as its own process, deletes 3 (the fence, the segment with the freed extent, the segment freeing it) and leaves the record, the checkpoint and the lease; --branches lists the branch.
- macOS: FXDAX_TEST_PARTS="workspace shuttle" scripts/test.sh --no-fail-fast: workspace 2061/2061 passed, every Shuttle suite passed.
- Linux (Docker via OrbStack, rust:1, CARGO_TARGET_DIR=/target/t165), not GCE: workspace 2231/2240 passed. The 9 failures, none in extent_remote: environment only (stacks_name_where_threads_wait, fxdaxctl_writes_its_stacks_on_sigquit, the_daemon_writes_its_stacks_on_sigquit: eu-stack missing; builds_are_stamped_with_their_commit; three grants tests: ENOENT), plus guests_forks_failing_at_each_step_harm_nothing (TASK-157's known failure under load) and serves_hundreds_of_forks_of_a_base (64 threads under load). Both passed when rerun alone in the same container. The Shuttle part (skipped by set -e after the workspace part failed) then passed in full.
- scripts/upstream-changes.py --check passes. DoD #3: extent_remote is fxdax's own crate; no upstream code changed.
- DoD #1 left unchecked: Linux was run in Docker as asked, not on GCE (scripts/test-gce.sh).

GCE r581 on main f5a5a9d82: Linux workspace 2249/2249, Shuttle and the other parts all pass; macOS workspace 2054 (rebased) and 2068 (with 160.8).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A bucket in a directory of the host's, file:///DIR (extent_remote::local): object_store's LocalFileSystem with conditional updates (and overwrites and deletes) under a flock of the bucket's, plus one conformance suite run by every bucket. Verified on macOS (workspace and Shuttle) and on Linux in Docker (workspace: only environment failures and two TASK-157 load failures that pass alone; Shuttle green). The suite and the branch lifecycle pass against RustFS and Azurite in Docker, and fxdax-gc collects a file bucket from another process.
<!-- SECTION:FINAL_SUMMARY:END -->
