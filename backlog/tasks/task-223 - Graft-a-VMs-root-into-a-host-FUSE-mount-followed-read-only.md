---
id: TASK-223
title: 'Graft a VM''s root into a host FUSE mount, followed read-only'
status: To Do
assignee: []
created_date: '2026-11-07 03:43'
labels:
  - fuse
  - host
  - followers
dependencies:
  - TASK-222
priority: low
type: feature
ordinal: 658000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Follow-up of TASK-222. The user wants one FUSE mount on the host showing a whole tree, read-write, with a VM's root at a subpath P: the VM writes P, and the host sees P as a live read-only follow of what the VM writes (or, if that is too hard, P is opaque from the host). One Fxfs has one writer (its journal and allocator), so P is its own store grafted into the FUSE namespace (crossing it EXDEV, writes under it EROFS), not a directory of the host's Fxfs. The followers' machinery exists (crates/fxfs/src/follower.rs; crates/follower's protocol, generic over a Kernel trait; RemoteExtentStore::connect_read_only, advance, pin). A host follower needs a storage device over a read-only RemoteExtentStore and a Kernel over FUSE, whose cache invalidation (FUSE_NOTIFY_INVAL_*) under the fill barrier can deadlock with reads holding page locks: a first version may serve P uncached. Carving P out of an existing host directory: fork the host store, prune all but P in the fork, boot the VM on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A host FUSE mount can graft another store at a path, shown read-only (EROFS under it, EXDEV across it)
- [ ] #2 A grafted store a VM writes is followed: what the VM syncs appears under the graft without remounting
- [ ] #3 The design of cache invalidation under the followers' protocol is recorded before it is built
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
