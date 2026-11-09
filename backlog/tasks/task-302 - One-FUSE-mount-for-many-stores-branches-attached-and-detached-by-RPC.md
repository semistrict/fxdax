---
id: TASK-302
title: 'One FUSE mount for many stores: branches attached and detached by RPC'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 11:38'
updated_date: '2026-11-09 13:05'
labels:
  - fuse
  - host
dependencies: []
priority: medium
ordinal: 715000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax-mount mounts one store per OS mount. The user wants one host FUSE mount into which branches are attached and detached on demand, since thousands of OS-level mounts are a bad idea: an explicit Attach takes a branch's lease (as fxdaxctl take / CreateStore do) and shows its filesystem at MOUNTPOINT/<name>/; Detach writes it back, closes it and lets the lease go, so a VM may take the branch again. Control is gRPC over a Unix socket, as the extent daemon's control API is. Supersedes the opaque-file and xattr ideas discussed around TASK-223.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxdax-mount --control SOCKET --stores DIR [--bucket URL] MOUNTPOINT serves one FUSE mount whose root lists the attached stores by name, empty at first
- [x] #2 Attach(name, branch, capacity, token) takes the branch into DIR/<name>, or reopens the store there, and shows its filesystem at MOUNTPOINT/<name>/; branch 0 makes a new one, its root the mounting user's
- [x] #3 Detach(name) takes the name away at once and returns once the kernel has let go of every node of the store, which is then unmounted, written back and its lease let go; with force, at once, held nodes failing ENOTCONN
- [x] #4 List returns the attached stores, their branches, and those detaching
- [x] #5 Node IDs and inode numbers of different stores never collide; rename and link across stores fail with EXDEV; nothing is made in the root (EPERM); statfs is per store
- [x] #6 fxdax-mount STORE MOUNTPOINT keeps working as before
- [x] #7 Tests: server tests of two stores attached, detached (waiting and forced) and reattached; a Linux end-to-end test attaching two branches over the RPC, writing, detaching with a file open, and reattaching from the store and from the bucket
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fuse::Server gets a second shape: a root directory of its own holding stores by name, each in a slot; node IDs = slot<<48 | object ID (EOVERFLOW above 48 bits); one-filesystem servers keep object IDs with the root swap.
2. attach(name, open) reserves name+slot, awaits the open; detach marks it detaching, sends FUSE_NOTIFY_INVAL_ENTRY (root, name) so the kernel drops its dentries and forgets the nodes, waits for the forgets (or not, with force), unmounts. Slots are reused only once no node of them is held.
3. Storage::take(path, (branch, capacity), bucket, token): CreateStore's take of a branch from a named bucket, or reopen.
4. MountOptions::mount_stores -> StoresMount (shared Served plumbing with Mount); the device fd is the notifier.
5. proto fxdax/mount/v1 (Attach/Detach/List), fuse::control (tonic server + blocking client), Go bindings regenerated.
6. fxdax-mount --control/--stores/--bucket, plus attach/detach/list subcommands.
7. Tests: fuse::server::stores_tests; tests/fuse_mount.rs one_mount_attaches_and_detaches_many_stores, run in a privileged Linux container and CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The first end-to-end run hung on a detach with a file open: telling the kernel to drop the store's name in the root unhashes that dentry alone; a dentry in use under it stays hashed, and once let go of stays cached, holding its parents and every node up the chain, so the kernel never forgets them. Fixed by keeping, in a server of many stores, the names the kernel was given each node under (kept through renames, unlinks and forgets), and invalidating them all on detach; names given while detaching are given for no time (entry_valid 0), so dentries made meanwhile go at their last use. fuse::server::stores_tests checks the names told (after a rename and an rmdir) and the zero validity.

Validation: macOS fxdax 330/330; fuse:: lib tests 7/7 (macOS, Linux); ubu1 (x86_64, non-root, fusermount3) fuse_mount + fuse_differential 8/8, seven runs; OrbStack container (arm64, root) fuse_mount 6/6 alone. ubu1 full workspace (scripts/test.sh clippy workspace): 2399/2452, the 53 failures environmental (46 userfaultfd without vm.unprivileged_userfaultfd, grants/jail tests needing root and other users, extentd tests, a page-cache check on ZFS) but one: fuse_differential's root reading mode 0 once under load, filed TASK-306. scripts/upstream-changes.py --check passes. Go bindings regenerated (go/generate.sh --check passes).
<!-- SECTION:NOTES:END -->
