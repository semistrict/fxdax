---
id: TASK-196
title: >-
  A daemon killed mid-ImportImage leaves a half-imported store that opens, and
  every import to its path after says ALREADY_EXISTS
status: To Do
assignee: []
created_date: '2026-11-06 06:46'
labels:
  - review-2026-11-06
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An import takes minutes (proto: "an import for minutes"), so a daemon restart, an upgrade, an OOM kill or a host crash mid-ImportImage is a likely production event. make_store (crates/fxdax/src/import.rs:75) removes a store it fails to fill only through Unfinished's Drop, which a killed process never runs. What is left at the path is a store whose description was written first (host_store::make, crates/fxdax/src/host_store.rs:180) and whose durable commits hold part of the image. host_store::unmade (crates/fxdax/src/host_store.rs:205) does not call it unmade, so:

- Storage::open and ExtentStore::open open it, and Fxfs mounts it as a filesystem holding part of the image (shown by the test below).
- extentd's import (crates/fxdax/src/extentd/stores.rs, `import`) and CreateStore answer EEXIST/ALREADY_EXISTS for the path. A VM manager that retries an import cut off by the restart cannot tell that from the whole image. If it takes ALREADY_EXISTS as done, makes the store public and forks VMs from it, they boot from a partial image.

CreateStore has the same gap with a smaller window: a crash between create_dir and the description leaves a directory that `create` refuses with EEXIST, and a VM process's OPEN with OPEN_CREATE (open_store with a capacity, stores.rs) tries to open and fails. Only ForkFrom calls removed_if_unmade (server.rs open_held).

Failing test, committed on branch review-extentd as #[ignore = "TASK-N"]: crates/fxdax/src/import.rs, `a_store_whose_importer_died_is_made_anew`. It runs a child test (`child_dies_importing`) that fills a store with one file, syncs it, prints FILLED and waits; the parent SIGKILLs it. On main it fails at "the store half made opens". Run it: cargo nextest run -p fxdax --lib --run-ignored all -E 'test(=import::tests::a_store_whose_importer_died_is_made_anew)'

Not fixed in the review: the fix changes how a store directory says it is whole (extent_store/host_store opening), which TASK-165.9 is rewriting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store whose import was killed mid-fill does not open as a store: Storage::open, ExtentStore::open and extentd's opens refuse it
- [ ] #2 ImportImage and CreateStore to a path an import or create left unmade remove what was left and make the store anew, as ForkFrom already does
- [ ] #3 a_store_whose_importer_died_is_made_anew passes on macOS and Linux with its #[ignore] removed, and goes red with the fix reverted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
