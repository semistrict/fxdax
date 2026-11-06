---
id: TASK-24
title: >-
  Image device: an I/O future leaked with mem::forget lets its buffer be reused
  while the kernel writes it
status: To Do
assignee: []
created_date: '2026-10-10 14:41'
updated_date: '2026-11-06 00:47'
labels: []
dependencies: []
type: bug
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in the pre-commit review of TASK-17/18. Of its four findings three went with virtio-fs: the FUSE server (crates/fxdax/src/fuse, deleted in 610923b34) and libkrun's virtio-fs device (vendor/libkrun, removed in 5ba394977, TASK-40), so a remount after DESTROY, INIT's max_pages and DAX-window descriptors in replies no longer exist to fix.

Left: the image device's I/O (crates/fxdax/src/storage/io.rs:43-52, Io::run): an I/O future leaked with mem::forget lets its buffer be reused while the kernel still writes it. Io::run's unsafe contract says not to leak it, but ImageDevice implements storage_device's safe Device::read_with_opts over it (crates/fxdax/src/storage/image.rs:121, 149-163). Consider owning bounce buffers or a different contract.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The image device's buffers stay valid even if an I/O future is leaked, or the decision not to is recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: rewritten to its one finding left. The DESTROY remount, INIT's max_pages and DAX-window descriptor findings, and their acceptance criteria, went with virtio-fs (crates/fxdax/src/fuse deleted in 610923b34; vendor/libkrun removed in 5ba394977, TASK-40).
<!-- SECTION:NOTES:END -->
