---
id: TASK-50.6.1
title: >-
  Guest Fxfs: 'Object id missing: Filesystem inconsistency' under containerd's
  overlayfs work
status: To Do
assignee: []
created_date: '2026-10-20 12:12'
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
- [ ] #1 The error is reproduced by a guest check that does what overlayfs's fallback does (mknod whiteouts, RENAME_EXCHANGE, unlink, concurrent lookups), or by looping the Docker case
- [ ] #2 Its cause is found and fixed, with the reproducer as a regression test
- [ ] #3 Optionally RENAME_WHITEOUT is supported, so overlayfs needs no fallback
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
