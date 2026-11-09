---
id: TASK-304
title: >-
  FUSE mounts made by root in an OrbStack container never end after unmount when
  fuse_differential runs beside fuse_mount
status: To Do
assignee: []
created_date: '2026-11-09 12:51'
labels:
  - fuse
  - flaky
dependencies: []
priority: low
ordinal: 716000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-09 on TASK-302's branch (d68181268), in a privileged fxdax-linux-rust container on OrbStack (arm64, kernel 7.2.12-orbstack), running as root (mount(2), not fusermount3): `cargo nextest run -p fxdax --test fuse_differential --test fuse_mount` hung 38 min with a_store_mounted_is_not_mounted_again, one_mount_attaches_and_detaches_many_stores and random_calls_on_a_mount_come_to_what_they_do_on_tmpfs each waiting for their FUSE workers, which sat in fuse_dev_do_read: no fuse mount was left in /proc/mounts, yet four connections stayed in /sys/fs/fuse/connections (waiting=0), so the kernel never gave ENODEV. Same mount namespace, no open fds or cwd in them, root mount private. fuse_mount alone passed there; on ubu1 (x86_64, Ubuntu 26.04, non-root through fusermount3) both binaries together pass (8/8). Unknown whether it is OrbStack's kernel, root's mount(2) path, or a real reference leak that ubu1's path hides.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reproduced or ruled out on ubu1 as root (mount(2) path), with both test binaries together, several rounds
- [ ] #2 If it reproduces, the reference that keeps the superblock alive is found and fixed, with a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
