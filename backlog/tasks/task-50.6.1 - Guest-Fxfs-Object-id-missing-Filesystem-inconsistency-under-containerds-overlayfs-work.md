---
id: TASK-50.6.1
title: >-
  Guest Fxfs: 'Object id missing: Filesystem inconsistency' under containerd's
  overlayfs work
status: Done
assignee: []
created_date: '2026-10-20 12:12'
updated_date: '2026-10-20 14:59'
labels: []
dependencies: []
parent_task_id: TASK-50.6
priority: high
type: bug
ordinal: 156000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
During fxfs-guest-docker-dax (guest booted on an Fxfs root, DAX), while containerd imported an image and ran a container with overlayfs snapshots over Fxfs, the guest kernel logged once 'fxfs: Object id missing: Filesystem inconsistency' (GCE run jail5, console). Nothing failed visibly: the container ran. overlayfs had warned that the upper fs does not support RENAME_WHITEOUT (Fxfs's rename takes RENAME_EXCHANGE and RENAME_NOREPLACE only), so it falls back to whiteouts made by mknod and renames. Suspects: a lookup or getattr racing an unlink or rename-exchange, or a dentry kept for an object Fxfs already removed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The error is reproduced by a guest check that does what overlayfs's fallback does (mknod whiteouts, RENAME_EXCHANGE, unlink, concurrent lookups), or by looping the Docker case
- [x] #2 Its cause is found and fixed, with the reproducer as a regression test
- [ ] #3 Optionally RENAME_WHITEOUT is supported, so overlayfs needs no fallback
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: rmdir (or a rename over a directory) tombstoned the directory's object at once (Fxfs::replaced, as upstream's fuchsia/directory.rs), while the kernel still held its inode dirty: times changed by an unlink in it, xattrs set (overlayfs's work dir, emptied, marked opaque, removed). Its write_inode (fsync of it, of a file unlinked from it, or the flusher) then hit txn_get_object_mutation's 'Object id missing'. Fix (commit 5c97e17): a removed directory stays in the graveyard, where replace_child put it, until fxfs_vfs_evict_inode frees it (evict's unlinked = !i_nlink for directories too). Reproducer: fxfs-guest-checks' removed_directories; fxfs-guest-docker-dax now fails on any 'Filesystem inconsistency' in the kernel log. On GCE with the old kernel both checks (and Docker) failed with exactly this error; with the fix, fxfs-guest-checks(-dax), fxfs-guest(-dax), fxfs-guest-docker-dax, fxfs-guest-daemon-lost pass. AC3 (RENAME_WHITEOUT) not done: optional, overlayfs's fallback now works. DoD1: the change is guest-only (Linux guest kernel); workspace untouched.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Removed directories now wait in the graveyard until the kernel evicts them, as unlinked files do, so writing back a removed directory's inode no longer finds its object gone ('Object id missing'). Reproduced by fxfs-guest-checks' removed_directories and the Docker case's new kernel-log assertion (both red on the old kernel, green with the fix, GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
