---
id: TASK-149.37
title: >-
  Random file system calls through the FUSE mount, against a Linux filesystem
  beside it
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 05:20'
updated_date: '2026-11-07 08:16'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: high
type: task
ordinal: 660000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host's Node layer (crates/fxdax/src/filesystem.rs), which imports and now FUSE mounts (TASK-222) go through, plays Linux's VFS for Fxfs's POSIX layer, and has only example tests: nothing compares its semantics with Linux's. TASK-222 found it clearing a directory's setgid on chown, which Linux never does. With a store mounted through the kernel, the same random sequence of ordinary calls (open/creat with flags, mkdir, mknod, symlink, link, unlink, rmdir, rename and renameat2 flags, chmod, chown, utimensat, truncate, fallocate, read, write, pwrite past the end, xattrs, readdir, statx, fsync, setgid directories, umask) can run on the mount and on a tmpfs or ext4 directory beside it, as the guest's fsx differential runs against ext4 (TASK-149.12), comparing results, errnos and the trees after: Linux's own filesystem the oracle. Linux only; runs in CI's workspace part as the runner's user, through fusermount3, and as root on GCE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A seeded, shrinking differential test runs random sequences of file system calls on a FUSE mount of a new store and on a directory of the host's own filesystem, and compares every call's result and errno, and the trees, attributes and contents after
- [x] #2 Differences Linux allows (inode numbers, st_blocks, times' granularity, d_type of special files) are listed in one place, each with why
- [x] #3 Each difference it finds is fixed or listed as known with a task, and the test is shown to find a mutant of Node::setattr and of the FUSE server, red
- [ ] #4 It runs in CI on Linux as the runner's user and on GCE as root
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/fxdax/tests/fuse_differential.rs (Linux): proptest sequences of file system calls (open with flags, read/write/pread/pwrite past the end, ftruncate/truncate, fallocate, fsync, fstat, mkdir, mknod, symlink, link, unlink, rmdir, rename/renameat2, chmod, chown/lchown, utimensat, stat/lstat, readlink, readdir, access, xattrs, umask, and as root the acting user switched per thread with setfsuid/setfsgid/setgroups) on a FUSE mount of a new store and on a tmpfs directory (/dev/shm, nodev) beside it.
2. Each call's result and errno compared; after each, both trees (types, permissions, owners, links, sizes, hard links, contents, targets, xattrs) compared, and which mtimes and ctimes the call changed, the reference's coarse clock ticked first; at the end the store unmounted, checked, mounted directly and its tree compared with the reference.
3. ALLOWED: one table of the differences Linux allows or the mount knowingly lacks, each with why (and a task for those it lacks); the comparison and generator consult it.
4. Runs as the user (fusermount3, CI's workspace part) and as root (GCE; Lima with sudo), cases from FXDAX_FUSE_DIFFERENTIAL_CASES.
5. Bugs found: fixed with a test for the class, red with the bug back, own commits. Mutants of Node::setattr and the server shown red.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: crates/fxdax/tests/fuse_differential.rs (Linux). proptest cases of 1-63 calls (open with flags, close, read, write, pread, pwrite past the end, lseek, ftruncate, truncate, fallocate, fsync/fdatasync, fstat, fchmod, mkdir, mknod FIFO/socket/devices, symlink, link with and without AT_SYMLINK_FOLLOW, unlink, rmdir, renameat2 with NOREPLACE, chmod, fchownat with and without NOFOLLOW, utimensat, stat/lstat, readlink, readdir, faccessat2 AT_EACCESS, set/get/list/removexattr l- and not, umask; as root, switching per thread between root and two users with setfsuid, setfsgid and the raw setgroups syscall) on a FUSE mount of a new memory store and on a tmpfs directory (/dev/shm for a user's run, root's own nodev tmpfs), on names a, b, c, d, one of 255 bytes and one of 256, three deep, the root too. Compared: every result or errno; both trees after each call (type, permissions, owner, group, links, size, device, which paths share a node, contents, symlink targets, xattrs); which mtimes and ctimes the call changed, the coarse clock ticked first; at the end the store unmounted, fsck'd, mounted directly and its tree compared with tmpfs's. ALLOWED lists what may differ and what isn't generated, each with why; those the mount lacks are TASK-226. A self-test, two_tmpfs_directories_agree. 256 cases by default (FXDAX_FUSE_DIFFERENTIAL_CASES), ~15 s as the user in Lima.

Bugs found and fixed (each its own commit, notes per bug):
1. setxattr/removexattr left the ctime (Linux's filesystems, the guest's fxfs_vfs_xattr_set, change it). Node played the guest's handler without it. Test: filesystem changes_of_status_change_the_ctime, red with the bug back.
2. The server took any xattr name; Linux's VFS resolves names to a filesystem's handlers (user, trusted, security for Fxfs): unknown namespace EOPNOTSUPP, a prefix alone EINVAL. FUSE leaves it to servers. Fixed in the server, not Node: imports write any name through Node (the guest reads system.posix_acl_* as ACLs). Test: fuse extended_attributes, red with the bug back.
3. A symlink longer than 256 bytes had size 0: fxdax keeps such targets in chunks, the record's link empty, and fxfs-posix's attr_of sized by link.len(). The guest sets i_size from the same attr: its long symlinks said 0 too. Blind spot: long-symlink tests read the target, never the size. Test: filesystem symlinks_are_as_large_as_their_targets (1..4095, made, looked up, remounted), red with the bug back. Fix: attr_of reads the chunks' length, or a symlink being made says it.
4. fallocate left mtime and ctime (Linux's file_modified, the guest's too). Test: changes_of_status_change_the_ctime, red with the bug back.
5. fallocate kept setgid where Linux kills it: with HANDLE_KILLPRIV_V2 the kernel tells servers only of writes, truncations and chowns, and a server can't know CAP_FSETID or the caller's groups, nor removed security.capability. Fix: KILLPRIV_V2 no longer offered; the kernel kills with the mode it computes; the server kills nothing itself. Tests: fuse init_negotiates, truncating_and_killing_privileges rewritten to the contract; the differential (fallocate, root's identities) red before.
6. chown(-1,-1) left the ctime: the kernel sends a SETATTR of nothing, Node::setattr changed nothing; Linux's notify_change always sets the ctime. Fix: every Node::setattr changes the ctime. Test: changes_of_status_change_the_ctime, red with the bug back.
Not bugs, listed in ALLOWED with why: FUSE's protocol limits (truncate(2) to a file's own size, sent as O_TRUNC is; a FIFO's I/O times; setgid without group-execute killed by Linux for one not of the group, not by FUSE's kernel), directory lseek, d_type of special files, inode numbers, st_blocks, directory sizes, atime, readdir order, non-UTF-8 names and times outside Fxfs's range (Fxfs's), and TASK-226's.
Blind spot (all): Node's VFS rules had example tests alone, each op's times and kills by hand; nothing compared with Linux. Looking around: every Node operation is now under the differential; the guest's own VFS callbacks have xfstests and pjdfstest. Structure: Node repeats Linux's per-operation rules as the guest's VFS callbacks do; the differential is the test of the class. Bug 2 and 5 point at FUSE's own division of work (what the kernel does for servers, what it leaves); kept in the server, with the decision in OPEN_QUESTIONS.

Mutants (red): Node::setattr keeping only 0o777 of a chmod's perm (chmod . 0o2775: perm 509 for 1533, at 300 cases); the server's mkdir not giving setgid in a setgid directory (3 of 3 runs at 256 cases, after mkdir modes were biased to setgid: at 40 cases, the old default, it survived 300 cases and was found at 3000).

Runs: Lima as user 3000 cases pass (340 s), as root 3000 pass (192 s).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
crates/fxdax/tests/fuse_differential.rs: random calls on a FUSE mount and on tmpfs, results, trees, times and the store compared, as the user and as root with identities switched; ALLOWED lists what may differ. It found six bugs, each fixed with its test (long symlinks sized 0, in the guest too; ctime left by xattr changes, fallocate and a setattr of nothing; any xattr namespace served; setgid kept over fallocate, by dropping KILLPRIV_V2); TASK-226 for the calls the mount refuses. Mutants of Node::setattr and the server red. Verified: Lima 3,000 cases as user and as root, macOS suite green. Left: AC 4's CI and GCE runs, which come with the push and the user's GCE run.
<!-- SECTION:FINAL_SUMMARY:END -->
