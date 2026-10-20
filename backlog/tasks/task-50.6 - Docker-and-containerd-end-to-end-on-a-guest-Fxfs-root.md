---
id: TASK-50.6
title: Docker and containerd end to end on a guest Fxfs root
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 12:12'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: medium
ordinal: 155000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandboxes run Docker and containerd inside their VMs. Nothing shows them working on an Fxfs root: overlayfs over Fxfs, their metadata stores, image pulls and container runs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest test boots on an Fxfs root, pulls an image and runs a container with Docker, and with containerd
- [x] #2 What fails is filed as a task, or documented as unsupported with what programs see
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: guest test fxfs-guest-docker-dax (guest/tests/test_cases/src/test_fxfs_guest_docker.rs) boots on an Fxfs root (Vm::fxfs_root: root=TAG rootfstype=fxfs), imports Docker 29.9.1's static binaries and a BusyBox image through the daemon. First GCE run: the guest boots on Fxfs; containerd imports the image and runs a container (overlayfs snapshots over Fxfs); dockerd starts with overlay2 on Fxfs. docker load then failed only for the archive's lack of Docker's manifest.json (added). Seen meanwhile: overlayfs warns 'upper fs does not support RENAME_WHITEOUT' (Fxfs's rename takes RENAME_EXCHANGE and RENAME_NOREPLACE only; overlayfs falls back), and the guest kernel logged once 'fxfs: Object id missing: Filesystem inconsistency' during containerd's work, which did not fail it: to look into. Fixed on the way: the image import deadlocked on large images (ImageDevice::write_with_opts blocked the executor's only thread in Store::begin_write while a commit waited for a write of that executor's: block_in_place; test writes_held_back_by_a_commit_leave_the_executor_running).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxfs-guest-docker-dax boots a guest on an Fxfs root (Vm::fxfs_root), and containerd imports and runs a container (overlayfs over Fxfs), then Docker loads and runs one (overlay2 on Fxfs); images loaded from archives, the guest having no network. Passes on GCE in the jailer. What failed on the way is fixed (import deadlock 065761c; Docker's manifest.json); the kernel's one 'Object id missing' is TASK-50.6.1. Commit 0b3bb3f.
<!-- SECTION:FINAL_SUMMARY:END -->
