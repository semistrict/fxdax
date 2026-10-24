---
id: TASK-26.6
title: Fxfs as a Linux filesystem in the guest kernel
status: Done
assignee: []
created_date: '2026-10-13 19:54'
updated_date: '2026-10-24 01:05'
labels: []
milestone: m-3
dependencies:
  - TASK-26.1
  - TASK-26.3
  - TASK-26.5
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: feature
ordinal: 42000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core of milestone 1 (doc-2): Fxfs's code, over the extent store, running as a filesystem in the guest kernel, so that metadata operations happen in guest memory and file data is read through DAX. Metadata is cached in guest memory and evicted like any cache; durability is POSIX with ordering preserved, a crash losing at most a suffix of history.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A libkrun guest mounts Fxfs from the extent-store device as its filesystem and passes the fxdax guest tests' filesystem checks and pjdfstest
- [x] #2 Lookups, stats, creates, renames and unlinks issue no device requests once metadata is cached
- [x] #3 After the VM is killed mid-workload, the filesystem mounts to a consistent prefix of its history, and fsync's ordering guarantees hold, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Progress (2026-10-14, branch object-store): Fxfs builds for the guest (41a7c5e) against a sysroot of the guest std (0119d2a); locks sleep on kernel futexes and stacks are 64 KiB (8d0d1dc); mount -t fxfs <tag> over the extent device (184281c); writes, links, renames, setattr, fsync, statfs, and formatting empty stores (ca1113e). Tests: fxfs-guest, fxfs-guest-format. Left: page cache (mmap, exec), refusing a second mount of one store, symlink owners, RENAME_EXCHANGE, xattrs, pjdfstest, crash test, x86-64.

2026-10-14 (branch object-store): AC2 met: bf0c5ac (lazy journal flush, Fxfs 0a90c57) - fxfs-guest counts the extent device's requests (sysfs) around 100 lookups/stats/creates/renames/unlinks: none. AC3 met: fxfs-guest-crash (libkrun) powers the VM off mid-workload with two threads fsyncing; the host's fsck is clean and every file whose fsync returned (140-180 per run) is whole. Also: page cache with mmap and exec (905f5f6), shared mounts (2b6cdd9), x86-64 kernel builds. Left for AC1: pjdfstest; the fxdax guest tests' filesystem checks against the guest's Fxfs.

2026-10-14: AC1 evidence (main 20b1ee4, libkrun fxdax branch): fxfs-guest-checks runs virtiofs-fxdax's guest and host checks on the guest's Fxfs: pass. pjdfstest-fxfs-guest: 237 files, 8792 tests, Result: PASS, with symlink/03.t skipped (PATH_MAX-1 symlink targets; Fxfs keeps targets in the symlink's record, at most 256 bytes). Left unchecked for the user: the skip, and that the guest mounts Fxfs at /mnt rather than as its root filesystem. DoD 1-3 hold (nextest macOS 1862/1862, Linux 1866/1866; upstream-changes --check passes; deviations commented in place).

2026-10-23 audit, the user agreeing: AC1's libkrun guest is now a Firecracker guest. Fxfs mounts from the extent device and is the guest's root filesystem where asked (vm.rs fxfs_root, TASK-50.6); pjdfstest's skip list is empty (test_pjdfstest.rs FXFS_SKIPPED). GCE r63, 2026-10-23, kernel vmlinux-drv3 at main 2428910c+ (guest code unchanged since): extent-guest, fxfs-guest, fxfs-guest-dax, fxfs-guest-checks, fxfs-guest-checks-dax, fxfs-guest-crash, fxfs-guest-crash-dax, pjdfstest-fxfs-guest, pjdfstest-fxfs-guest-dax: 9/9 OK, every device served by fxdax-extentd (daemon mode, the only one since 537c7eb4). Workspace nextest: macOS 2466 passed (scripts/test.sh), Linux 2544 passed (GCE). scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fxfs is a Linux filesystem in the guest kernel (fs/fxfs over the virtio extent device), passing the guest checks and pjdfstest with and without DAX on GCE. Closed in the 2026-10-23 audit of stale tasks.
<!-- SECTION:FINAL_SUMMARY:END -->
