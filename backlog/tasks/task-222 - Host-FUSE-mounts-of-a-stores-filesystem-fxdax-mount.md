---
id: TASK-222
title: 'Host FUSE mounts of a store''s filesystem: fxdax-mount'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 03:43'
updated_date: '2026-11-07 04:11'
labels:
  - fuse
  - host
dependencies: []
priority: medium
type: feature
ordinal: 657000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A store's Fxfs (an image, an imported base, a stopped VM's store) can only be reached from the host through the fxdax library (examples/store-read.rs, store-tree.rs) or by booting a VM. The user wants to mount it with FUSE on the host, as a plain filesystem, read-write, served by the host's Fxfs (fxdax::Filesystem). fxdax once had a FUSE server, for virtio-fs (TASK-17, deleted in 610923b34 and fda9d7d2 when the virtio-fs backend went); its protocol code and tests are in those commits' parents. The store must be one nothing else has open: its directory is flocked by whoever opens it. Linux only: macFUSE and FUSE-T are not open source, so macOS mounts are out of scope.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A FUSE server, independent of its transport, serves a mounted Filesystem: entries, attributes, file I/O, directories, symlinks, hard links, special files, renames, extended attributes, statfs, fsync, fallocate; malformed requests answered EINVAL, unsupported ones ENOSYS
- [ ] #2 Protocol tests drive the server with requests encoded by an independent client, on every host
- [ ] #3 On Linux, a mount through /dev/fuse: mount(2) as root, fusermount3 otherwise; served until unmounted, after which the filesystem is closed and its store whole (fsck)
- [ ] #4 fxdax-mount STORE MOUNTPOINT mounts a store; SIGINT or SIGTERM unmounts it and it exits 0
- [ ] #5 A Linux test mounts a store through the kernel, works on it with ordinary file system calls, unmounts, and finds what it wrote in the store, checked
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Node: ..'s inode (the held parent); setattr's kill_suid_sgid never touches a directory, as Linux's chown.
2. fxdax::thread (Linux): one helper that blocks signals in every thread and waits for them on its own; extentd's SIGTERM drain uses it.
3. fxdax::fuse::abi: the messages and flags used, from Linux's include/uapi/linux/fuse.h (7.36), fxdax's own, as TASK-17's were.
4. fxdax::fuse::Server: handle(request bytes) -> reply bytes or none. A table of the kernel's lookups (nodeid -> Node, count; FUSE_ROOT_ID and the root's object ID swapped), directory handles holding a snapshot of entries, xattrs, setgid directories' group inherited (the kernel's inode_init_owner, which FUSE leaves to the server). OPEN/FLUSH refused (ENOSYS: no open or flush sent again), caches long: the mount is the store's only writer.
5. fxdax::fuse::Mount (Linux): /dev/fuse; mount(2) with default_permissions, falling back to fusermount3 (_FUSE_COMMFD, SCM_RIGHTS) on EPERM; worker threads read, handle and reply until ENODEV; unmount (umount2 or fusermount3 -u, lazily on request); join closes the filesystem and gives the storage back.
6. bin/fxdax-mount STORE MOUNTPOINT [--allow-other]: SIGINT/SIGTERM unmount lazily; exits 0 once the filesystem closed.
7. Tests: tests/fuse.rs, the protocol through an independent client (every host); tests/fuse_mount.rs (Linux): through the kernel, in process and through the binary, then the store checked (fsck) and read back.
8. CI and GCE hosts install fuse3 (fusermount3); README and crate docs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: fxdax::fuse (abi.rs from fuse.h 7.36; server.rs, transport-independent; mount.rs, Linux: /dev/fuse, mount(2) else fusermount3 with _FUSE_COMMFD/SCM_RIGHTS, worker threads until ENODEV, Unmounter, join closes); bin/fxdax-mount; fxdax::signals (Signals::block/wait, spawn_taking_no_signals), which stacks.rs and fxdax-extentd now use; Node::parent; tests/fuse.rs (17 protocol tests, independent client), tests/fuse_mount.rs (4 tests through the kernel), fxfs-posix tests::mode_owner_and_group_change_one_at_a_time; fuse3 in CI, GCE and Lima packages; README, crate docs; dead-ends/fuse-writeback-cache.md updated; decisions in OPEN_QUESTIONS.md.

Bug 1, fxfs-posix setattr: a chown alone of a node without POSIX attributes (a new volume's root directory) made its mode 000. Found by fuse_mount's first run in Lima: every write to a new store's mount, whose root the test chowned to its user, EACCES. Fxfs's update_attributes makes POSIX attributes of the fields given, the rest 0, where a node has none; attr() reports defaults (0o755, uid 0) for such a node, so nothing showed it. Blind spot: no test changes one of mode/uid/gid alone on a node without POSIX attributes: Node and the guest's VFS create every node with all three, the root alone has none, and the model tests have no chown/chmod op. Test for the class: tests::mode_owner_and_group_change_one_at_a_time, each of the three alone, on the root and on a file that has them; red before the fix ((0o40000, 7, 0) for a chown). Fix: setattr fills the fields not given from attr_after under the transaction's lock. Looking around: fxfs-posix writes POSIX attributes in create, symlink and tmpfile with all three, and in setattr alone partially: no other writer. Structure: one writer of a node's POSIX attributes that always writes all three would make the class impossible; setattr is now that writer for changes, creations already were.

Bug 2, Node::setattr: kill_suid_sgid cleared a directory's setgid (and setuid), which Linux never does (chown_common sets ATTR_KILL_SUID only for non-directories): a chown through the FUSE server (HANDLE_KILLPRIV_V2 sends every chown as kill) would have cleared a shared directory's setgid. Found reading the old server for this port. Blind spot: the kill tests ran on files alone. Tests: filesystem setattr_changes_owner_permissions_and_times and fuse truncating_and_killing_privileges chown a setgid directory; both red with the bug back. Looking around: the guest's kill path is the kernel's own (setattr_should_drop_suidgid), not this one. Structure fine: the rule lives in Node::setattr, the one place.

Validation so far: macOS fuse, filesystem, signals tests 64/64; fxfs-posix 75/75; Lima (aarch64 Ubuntu 24.04) fuse_mount 4/4 as a user (fusermount3) and 4/4 as root (mount(2)), fuse protocol 17/17, signal/stack/sigterm tests pass.
<!-- SECTION:NOTES:END -->
