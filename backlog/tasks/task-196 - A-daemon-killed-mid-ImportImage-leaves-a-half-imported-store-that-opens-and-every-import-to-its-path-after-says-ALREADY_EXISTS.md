---
id: TASK-196
title: >-
  A daemon killed mid-ImportImage leaves a half-imported store that opens, and
  every import to its path after says ALREADY_EXISTS
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:46'
updated_date: '2026-11-07 09:26'
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
- [x] #1 A store whose import was killed mid-fill does not open as a store: Storage::open, ExtentStore::open and extentd's opens refuse it
- [x] #2 ImportImage and CreateStore to a path an import or create left unmade remove what was left and make the store anew, as ForkFrom already does
- [x] #3 a_store_whose_importer_died_is_made_anew passes on macOS and Linux with its #[ignore] removed, and goes red with the fix reverted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Taken into TASK-165.9's opening path (the user, 2026-11-06). A store directory says it is whole only once it is: create_host writes an 'unmade' marker, durably, before the description; its maker removes it, durably, once the store is whole (CreateStore at once; an import once its image is filled and synced). Opening (extent_store's recovery) refuses a marked store; host_store::unmade reports it; extentd's ImportImage and CreateStore remove what an import or create left unmade and make it anew, as ForkFrom does. Un-ignore a_store_whose_importer_died_is_made_anew; check it red with the fix reverted.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in 1597b18aa with TASK-165.9 (as the user asked): Store::make writes the 'unmade' marker before anything else, made() removes it once the maker filled the store (an import once it synced, a create at once); Store::recover, Storage::open, ExtentStore::open and extentd refuse an unmade store (extent_store::Unmade, ENOENT); ImportImage and CreateStore remove what an import or create left unmade and make it anew (stores.rs removed_if_unmade). a_store_whose_importer_died_is_made_anew is un-ignored and passes; stores_unmade_open_not (host_store) and stores_unmade_refuse_to_open (extent_store) cover the refusals. Left for its closing with TASK-165.9.

Verified: CI 42352682020 on land-9 (landed as c9207e5d4) green, Linux workspace included; macOS workspace 2079/2079 (scripts/test.sh at c9207e5d4); scripts/upstream-changes.py --check passes. a_store_whose_importer_died_is_made_anew (crates/fxdax/src/import.rs) runs un-ignored in both.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store says it is whole last: Store::make writes 'unmade' first and made() removes it; Storage::open, ExtentStore::open and extentd refuse an unmade store (ENOENT), and ImportImage and CreateStore remove one and make it anew. Landed with TASK-165.9; a_store_whose_importer_died_is_made_anew passes un-ignored. Verified by CI 42352682020 and the macOS workspace.
<!-- SECTION:FINAL_SUMMARY:END -->
