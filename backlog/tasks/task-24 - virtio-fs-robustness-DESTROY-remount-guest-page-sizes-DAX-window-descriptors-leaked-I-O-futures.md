---
id: TASK-24
title: >-
  virtio-fs robustness: DESTROY remount, guest page sizes, DAX-window
  descriptors, leaked I/O futures
status: To Do
assignee: []
created_date: '2026-10-10 14:41'
labels: []
dependencies: []
type: bug
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in the pre-commit review of TASK-17/18. (1) crates/fxdax/src/fuse/mod.rs: DESTROY unmounts the filesystem for good, but virtio-fs sends it on every guest umount, so a remount passes INIT and then gets ENOTCONN until the VMM restarts; DESTROY should forget all nodes and handles and stay mounted, leaving the unmount to closing the server. (2) max_pages in INIT is computed from the host page size, so a guest with larger pages (64K arm64 on a 4K host) can send READs above MAX_BUFFER_SIZE, which fail with ENOMEM. (3) vendor/libkrun/src/devices/src/virtio/fs/fxdax.rs buffers(): guest descriptors may point into the DAX window, which is part of GuestMemoryMmap; on Linux a reply copied into an unmapped window page faults on a VMM thread that userfaultfd can't resolve (dax/uffd.rs aborts). Reject descriptors outside RAM regions. (4) crates/fxdax/src/storage/io.rs: an I/O future leaked with mem::forget lets its buffer be reused while the kernel still writes it; the unsafe contract says not to leak, but Device::read_with_opts is safe. Consider owning bounce buffers or a different contract.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest can unmount and remount the filesystem in one VMM session, shown by a guest test
- [ ] #2 INIT's max_pages fits MAX_BUFFER_SIZE for guest pages up to 64 KiB
- [ ] #3 Descriptors outside guest RAM are refused, shown by a test
- [ ] #4 The image device's buffers stay valid even if an I/O future is leaked, or the decision not to is recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 libkrun guest tests pass on macOS and Linux
<!-- DOD:END -->
