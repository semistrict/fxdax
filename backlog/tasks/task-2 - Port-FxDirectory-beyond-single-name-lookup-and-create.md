---
id: TASK-2
title: Port FxDirectory beyond single-name lookup and create
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 04:01'
labels: []
milestone: m-0
dependencies: []
references:
  - vendor/fuchsia/src/storage/fxfs/platform/src/fuchsia/directory.rs
type: feature
ordinal: 2000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Step 5 ported only what paged_object_handle tests need: FxDirectory can look up and create one name. A filesystem served to a guest also needs readdir, unlink, rmdir, rename, mkdir, symlinks, hard links and directory attributes. Upstream directory.rs implements these over fuchsia.io; port them verbatim through the in-process facade, documenting any deviation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 FxDirectory supports readdir, unlink, rmdir, rename (same and cross directory), mkdir, symlink, readlink and link
- [x] #2 Upstream directory.rs tests for these operations run and pass (disabled ones list their reason in kernel-cfg.py)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Approach: build upstream directory.rs (and fxfs symlink.rs) from vendor via kernel-cfg.py, as paged_object_handle.rs is, so its tests run nearly verbatim. User left the choice to me (vendored + cfg over a larger hand excerpt).

1. fidl stand-in: connections whose server can be set after the call is made (pipelined open), epitaphs, and protocol-erased channels, as Directory.Open needs.
2. fuchsia.io stand-in: Directory (open, unlink, rename, link, get_token, read_dirents, create_symlink, attributes, sync, query, clone, xattrs) and Symlink protocols.
3. vfs: vendor path.rs, directory/traversal_position.rs, directory/dirents_sink.rs (and token_registry.rs if it builds) verbatim; excerpt entry_container traits and Symlink; stand-ins for ObjectRequest and the mutable directory and symlink connections.
4. fuchsia-fs: directory helpers the tests use (parse_dir_entries, readdir, clone).
5. fxfs-platform: directory.rs and symlink.rs from vendor with cfgs disabling fscrypt, casefold, watchers, block devices and SELinux; dirent_cache.rs verbatim; volume.rs excerpt gains the dirent cache, purge, deleted-directory marking and symlink loading; FxFile connections take ObjectRequest like upstream; testing.rs open helpers become upstream verbatim.
6. Add fxdax tests for operations upstream only covers in fs conformance suites (rename same/cross directory), if not covered.
7. Run workspace tests on macOS and Linux; kernel-cfg --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- directory.rs and symlink.rs are built from vendor by kernel-cfg.py (+ directory-no-watchers.patch); dirent_cache.rs verbatim.
- vfs: path, dirents_sink, traversal_position, read_dirents, directory/common, request_handler vendored; token_registry and entry_container via kernel-cfg (no watchers); name crate vendored.
- Connections run upstream's RequestListener over an in-process request queue, so requests on a connection are handled in order as upstream (fixed open/unlink ordering failures).
- fidl stand-in: pipelined opens (one-way calls queue until served), epitaphs (upstream Epitaph), events (OnRepresentation).
- Compiled-out upstream tests, with reasons in kernel-cfg.py: watchers, fuchsia.io xattr methods, SELinux via node connections, fdio. query_filesystem (FilesystemInfo) not ported yet; needed for statfs in TASK-5.
- Added crates/fxfs-platform/src/fuchsia/directory_tests.rs: cross-directory rename (file, over file, directory), rename into descendant, cross-directory link, persistence across remount.
- Validation: scripts/test.sh macOS 1707 passed; Linux (Lima) 1706 passed; kernel-cfg.py --check and upstream-changes.py --check pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FxDirectory and FxSymlink are now upstream's directory.rs/symlink.rs built from vendor (kernel-cfg + one patch), served over an in-process fuchsia.io whose vfs connections are upstream's handlers on upstream's RequestListener. Upstream directory tests run (watcher/xattr/SELinux/fdio ones compiled out with reasons); fxdax tests cover cross-directory rename/link and persistence. Verified: 1707 tests pass on macOS, 1706 on Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
