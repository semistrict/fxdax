---
id: TASK-42.6
title: >-
  Followers in the guest kernel: read-only mounts, DAX and page-cache
  invalidation, dentry revalidation
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 22:17'
updated_date: '2026-10-24 00:53'
labels: []
dependencies:
  - TASK-42.3
  - TASK-42.4
  - TASK-42.5
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 126000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest side (doc-8, Concurrency 2): fs/fxfs honours -o ro and a read-only device, and turns the follower's change sets into invalidation under the kernel's own locks, DAX first (the primary path).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fs/fxfs mounts read-only on -o ro or a read-only device, writing nothing; the driver refuses writes on a read-only device
- [ ] #2 After a batch, changed DAX ranges' user mappings and DAX entries are broken under the inode's invalidate_lock, page-cache ranges invalidated, sizes and attributes refreshed, directory generations bumped and stale dentries dropped by d_revalidate
- [ ] #3 WARN_ON invariants in debug builds (doc-8 testing 1); the kernel patch series in guest/linux/patches carries the change
- [ ] #4 No kernel cache is filled from a read outside the follower's fill barrier (vfs.h), which replaced the seqcount by TASK-42.10's decision
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Device: driver offers VIRTIO_EXTENT_F_READ_ONLY, refuses writes on it (-EROFS), sends ADVANCE/LET_GO(epoch)/PIN/UNPIN, reads committed; fs/fxfs's device glue says FXFS_DEVICE_READ_ONLY and maps sealed extents read-only for DAX; Rust ExtentDevice implements Device::{advance, pin, unpin} (LET_GO from Previous's drop, naming its epoch). 2. Rust (fxfs-linux): mount read-only on -o ro or a follower's device; a follower runs follower::Protocol over GuestKernel (C shims): tree reads (lookup, getattr, readdir in validated chunks, readlink, xattrs, DAX extents, read with a grace section) validated; no handle caching on followers; the batch (fxfs_follow) applies, reads changed objects' attributes inside the seqcount, invalidates through fxfs_vfs_changed; open files held (fxfs_hold/release). 3. C (super.c, dax.c): seqcount_t written with raw_write_seqcount_* under the follower's mutex (seqcount_mutex_t's write side disables preemption, and a batch sleeps), SRCU; follow work on the device's configuration change; invalidation per inode: data (DAX ranges dropped under invalidate_lock, inode or not; page cache pages), attributes (inode takes the batch's, grown files' ranges dropped), directories' generation bumped and d_revalidate against it; read(2) on DAX takes invalidate_lock shared; SB_RDONLY; reconfigure refuses rw. 4. Guest tests: fxfs-guest-follow(-dax): writer and follower VMs side by side, the follower sees a new file, a rename, a deletion (open file still reads), an overwrite through read(2) and its mapping. 5. Kernel patch series, doc-8, WARN_ON invariants.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Progress (2026-10-18): driver, device glue, Rust (follow.rs, fs.rs, lib.rs, device.rs) and C (super.c, dax.c, vfs.h, fxfs.h) written; the guest Rust checks locally against the sysroot (scratch: build-guest-sysroot + cargo check); the x86-64 guest kernel builds clean on GCE (6.18 dropped sb->s_d_op: set_default_d_op). Writer regressions fxfs-guest and fxfs-guest-dax pass on it. First fxfs-guest-follow run: the follower's mount failed, its journal replay reading the writer's next journal extent, which extentd answered EIO; fixed by 7a36a35 (ENOENT for extents written after the view, EIO for what it let go of).

2026-10-23 audit: AC3 reworded from the seqcount, which TASK-42.10 replaced with the fill barrier (a17eb735); its index is now #4. Evidence found for the others, unchecked until a follower run on the current tree: driver.c returns -EROFS on writes of a read-only device and fxfs-guest-follow checks an MS_RDONLY mount gets EROFS (AC1); d_revalidate and invalidate_lock in super.c, c0c0f672, befc9c14 (AC2); WARN_ON_ONCE invariants in super.c and dax.c, and 0004-fxfs.patch carries fs/fxfs (AC4). Open follow-ups: TASK-79.23, 79.54, 79.30, 92.31.
<!-- SECTION:NOTES:END -->
