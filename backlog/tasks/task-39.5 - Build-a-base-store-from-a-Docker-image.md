---
id: TASK-39.5
title: Build a base store from a Docker image
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 00:30'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 115000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
There is no supported way to turn a Docker (OCI) image into a base store. A tool (fxdaxctl) should take an image (or its exported root filesystem) and write an Fxfs filesystem into a new store, with owners, modes, xattrs, hard links, symlinks and devices as the image has them, ready to fork.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxdaxctl builds a base store from an OCI image (or a docker export tarball), shown by a test that boots a guest on a fork of it and checks files, owners, modes, links and xattrs
- [x] #2 Building is reproducible enough to compare: the same image gives the same files
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Host Filesystem: symlinks' owners/times (FxSymlink::update_attributes) and xattr calls (getxattr/setxattr/listxattr/removexattr).
2. fxdax::import: Importer applies tar layers (gzip or not) per the OCI changeset rules (whiteouts, opaque dirs, implicit parents, hard links, devices, PAX xattrs/times/owners, holes); OciImage reads an OCI layout (index -> platform manifest -> layers), checking digests; import::store makes, fills and checks a store.
3. fxdaxctl import DIR CAPACITY IMAGE [REF].
4. Tests: host (crates/fxdax/tests/import.rs: attributes, layering, reproducibility, digests, path traversal, CLI); guest fxfs-guest-import(-dax): boot on a fork of an imported store, check files/owners/modes/links/xattrs/devices/whiteouts; base unchanged.
<!-- SECTION:PLAN:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdaxctl import DIR CAPACITY IMAGE [REF] (49fe64c, 21d93c9) makes a base store from an OCI image layout (skopeo copy / docker save; the platform's manifest of an index) or one layer (docker export), via fxdax::import: OCI changeset rules (whiteouts, opaque dirs), owners, modes incl. setuid/sticky, times (PAX fractions), hard links, symlinks (host Filesystem gained symlink owners/times and xattr calls), devices, FIFOs, PAX SCHILY.xattr, holes; digests checked; '..' refused; a failed import deletes only what it made. Reproducible: same image -> same manifest (host test). Verified: crates/fxdax/tests/import.rs (6 tests), guest fxfs-guest-import and -dax (Lima arm64), macOS 1984 and Linux 2006 workspace tests, upstream-changes --check. Not supported: zstd layers, LIBARCHIVE.xattr records, docker save tarballs unextracted (extract to a dir first).
<!-- SECTION:FINAL_SUMMARY:END -->
