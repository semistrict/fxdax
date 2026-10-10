---
id: TASK-19
title: >-
  Own the Fuchsia sources: copy what fxdax builds into crates/, delete
  vendor/fuchsia
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 02:36'
updated_date: '2026-10-10 14:44'
labels: []
dependencies: []
type: chore
ordinal: 19000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax builds Fuchsia code from vendor/fuchsia (an unmodified copy at the commit in vendor/fuchsia/COMMIT, refreshed by scripts/vendor-fuchsia.sh) and from vendor/fuchsia-kernel (a generated mirror whose cfgs, disabled items and patches scripts/kernel-cfg.py applies). Fixing a bug in vendored code (TASK-15, fuchsia-async's scope) needed yet another patch through the mirror. The user decided to own the code instead: copy into crates/ only the Fuchsia sources the workspace builds, with their tests, and delete vendor/fuchsia, vendor/fuchsia-kernel and the scripts that maintain them. vendor/libkrun (a submodule) stays.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every Fuchsia source the workspace compiles on macOS and Linux (lib and test targets) lives under its crate in crates/, found from cargo's dep-info; nothing else from vendor/fuchsia is kept
- [x] #2 The kernel mirror's generated files are what is copied, so cfg rewrites and patches carry over, and their deviations from upstream are documented in the files
- [x] #3 No Cargo.toml, #[path], build script or include in the workspace refers to vendor/fuchsia or vendor/fuchsia-kernel; both directories are deleted, as are vendor-fuchsia.sh, kernel-cfg.py, kernel-patch-update.sh and scripts/kernel-patches
- [x] #4 The Fuchsia commit the copies come from is recorded, each copied crate cites its upstream source, and scripts/upstream-changes.py lists the code to update from a Fuchsia checkout instead of vendor/fuchsia
- [x] #5 Fuchsia's and Zircon's license notices are kept with the code
- [x] #6 README and docs describe the new layout
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 libkrun guest tests pass on macOS and Linux
- [ ] #6 The test count is unchanged
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Inventory: cargo check --workspace --lib --bins --tests --examples into fresh target dirs on macOS and Linux (Lima); the dep-info (.d) files name every vendored file compiled (212, the same on both).
2. Copy each into its crate: crates/<crate>/src/<path under the crate's upstream src dir>; files that crates pulled in by #[path] go where their module naturally lives. Mirror-generated files (vendor/fuchsia-kernel) are what is copied, so cfg rewrites and patches carry over.
3. Point Cargo.toml lib/bin paths and #[path] attributes at the copies; zx-sys owns definitions.rs with its extern block unconditional (no build script); zircon's build script reads it there.
4. Drop module declarations for Fuchsia-only files that were not copied (fuchsia-async's Fuchsia runtime, handles and sockets; fuchsia-sync's futex locks; storage_device's block device, pinned buffers and FileBackedDevice).
5. Document deviations in each differing file's leading comment (from kernel-cfg.py's rules and the patch descriptions); cite each crate's upstream dir in its Cargo.toml; record the commit in [workspace.metadata.fuchsia]; rewrite upstream-changes.py to diff a Fuchsia checkout; copy LICENSE/PATENTS/AUTHORS and Zircon's MIT license.
6. Delete vendor/fuchsia, vendor/fuchsia-kernel, vendor-fuchsia.sh, kernel-cfg.py, kernel-patch-update.sh, scripts/kernel-patches; update README, crate comments, backlog DoD.
7. Verify: every copy is byte-identical to upstream at the commit or notes its deviations; workspace suites on macOS and Lima (test count unchanged but for TASK-15's new test); guest tests on both.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: the 212 files cargo's dep-info lists for macOS and Linux (identical) were copied; 194 are byte-identical to Fuchsia at 90e54e09 and the other 18 (plus zx-sys's definitions.rs) state their deviations in their leading comment, checked by diffing every copy against ~/src/fuchsia. Workspace suites: macOS 1805 passed, 4 skipped; Linux (Lima) 1808 passed, 4 skipped: the counts before plus TASK-15's new scope test, with no warnings. Guest tests virtiofs-fxdax and -nodax pass on macOS and Linux. scripts/upstream-changes.py --check passes against the checkout. The fxdax_kernel cfg is kept in the copies (build scripts set it); resolving it would delete the compiled-out items, tests among them, so it is left for the user to decide.

Resolved the fxdax_kernel cfg at the user's request: #[cfg(fxdax_kernel)] attributes dropped, items under #[cfg(not(fxdax_kernel))] removed with their attributes and doc comments (the compiled-out tests among them: watcher, extended-attribute, SELinux and fdio tests in directory.rs, VMO-children and panic-message tests in pager.rs and paged_object_handle.rs), and the build scripts that only set the cfg deleted (fxfs, storage_units, storage_device, fxfs-platform; vfs's keeps the API levels). Deviation notes and README updated. Workspace suites with the TASK-15 stress runs going: macOS 1805 passed, 4 skipped; Linux 1808 passed, 4 skipped, no warnings: counts unchanged, as the removed tests never ran.

After resolving the cfgs: libkrun guest tests virtiofs-fxdax and -nodax pass on macOS and Linux (Lima, run from a separate copy while the TASK-15 stress run used ~/fxdax-test).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax owns the Fuchsia code it builds: only the files the workspace compiles (from cargo dep-info on both hosts), with their tests, now live in their crates under crates/; vendor/fuchsia, vendor/fuchsia-kernel, vendor-fuchsia.sh, kernel-cfg.py, kernel-patch-update.sh and the kernel patches are deleted. The mirror's cfg rewrites and patches carried over in the copies, and each copy that differs from upstream says how in its leading comment; Fuchsia-only modules that weren't copied are no longer declared. The Fuchsia commit is in [workspace.metadata.fuchsia], each copied crate cites its upstream dir, and upstream-changes.py diffs a Fuchsia checkout. Fuchsia's LICENSE/PATENTS/AUTHORS are at the root and Zircon's MIT license in crates/zircon and crates/page. Verified by suites and guest tests on macOS and Linux. Not committed.
<!-- SECTION:FINAL_SUMMARY:END -->
