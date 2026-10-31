---
id: doc-10
title: Fxfs overlays for shared repository checkouts
type: specification
created_date: '2026-10-30 09:08'
updated_date: '2026-10-31 06:25'
---
# Fxfs overlays for shared repository checkouts

Status: design, 2026-10-31. The guest's mounts of volumes are built (TASK-136.2); the rest isn't. Work: TASK-136. It sits on top of the filesystem and
is separate from TASK-96, the abstract extent store split. Idea note:
`~/docs/ideas/fxfs-native-overlay-for-shared-repos.md`.

Many users clone the same repos over and over and keep them up to date. Infrastructure should keep
a base with a full checkout current, and each sandbox should get a fork of it with its own writable
layer, mounted at a subpath in the VFS, not as the root, with overlay semantics from Fxfs
primitives.

## Decisions

Ramon's, 2026-10-30 and 2026-10-31:

| Question | Decision | Rejected |
|---|---|---|
| Scope | Any repo, attached at runtime | Repos known at sandbox start, on a second boot device: no runtime attach, no mixing of generations |
| Reading a base outside the branch's lineage | A device read-through to a pinned fork of the base: one device, one DAX window | A second read-only device, which needs per-extent routing in `dax.c` and is fixed at boot; a multi-parent graft, which changes the lineage, the bucket layout and collection |
| Tenancy | Public repos in public bases; each tenant's private repos in its own bases; no sharing across tenants otherwise | Public repos only at first |
| Attach authorization | The sandbox manager alone, through the host control API. The guest cannot ask | The guest asking through the device, with extentd checking grants |
| Repos mounted at once | Hundreds per sandbox: opening volumes on first use, expiring idle mounts and constant-cost reads are required | Tens, which would leave those as follow-ups; thousands, which would budget per-volume memory and flush loops too |
| Moving to a newer generation | Git level first: `git fetch` reaches a new generation's objects through alternates, and `git pull` writes what changed into the writable layers | An in-place swap of base layers now, which stays possible later if measurements justify it |

Proposals, not yet confirmed: new mounts taking the latest generation, and untouched repos advancing at their next mount; a publisher on the host, with Git in
process; the manifest below.

## Why not bake repos into a base store

A store fork already makes every volume private, with no `snapshot_volume`, and needs no store or
device change. But the base's whole size lands on each cold mount:

- Every mount opens every volume (`crates/fxfs/src/object_store/object_manager.rs:252`), loading
  each layer's seek table and bloom filter.
- Every mount scans the whole allocator tree into memory
  (`crates/fxfs/src/object_store/allocator.rs:1051`).
- Each volume's layers reserve 1.5 times their size (`object_store.rs:2043`), and each volume's
  tree cache takes the whole `object_cache_items` budget.

VM snapshots hide those mounts, up to the snapshot. Baking also ties a sandbox's root image to the
base's lineage, or takes a second boot device per base.

## The model

A repo in a sandbox is a volume of the sandbox's Fxfs whose bottom LSM layers are a base's,
read-only, mounted on a directory: one `fxfs` mount of one volume, with nothing stacked. The fork of
the base is never mounted; it only pins the extents and serves reads of them. The tree merges
records by key, tombstones hide base records, and writes make new extents, per block. So there are
no whiteouts, no merged directories and no file-level copy-up.

A graft alone would not attach anything. Base layers are objects in the base's root store, so the
guest needs this model however the extents are read. The store question is only how they are read.

## Design

### Guest VFS (TASK-136.2)

Built: `mount -t fxfs -o volume=NAME tag path`. One Fxfs on the device, one superblock per mounted
volume, so each repo has a native `st_dev` and `EXDEV`, and attaching needs no new device.

- `struct fxfs_vfs_filesystem` holds what a device's superblocks share, and in Rust `Filesystem`,
  which the last unmount closes. A follower still mounts its default volume alone.
- The DAX window is the device's. Each volume has its own table of files, as inode numbers collide
  across volumes. An unmount with others staying drops its ranges and leaves evacuation running.
- The filesystem freezes once all its superblocks are frozen, also when the last unfrozen one
  unmounts, and thaws with the first. No volume mounts while it is frozen (`EBUSY`).
- Stores' tree caches share one budget. Over it, each cache gives up its oldest item in turn, so
  idle volumes empty and active ones keep the room.
- With `Options::lazy_volumes`, on for the guest but not followers, a mount opens only volumes the
  journal tail names, and the rest open when mounted. A closed volume holds nothing in the journal.
  What opening adds to the metadata reservation is reserved then, not borrowed: a borrow persists,
  and the next mount's reservation, without that volume, would be less than what is borrowed.

Writing to a volume keeps it in the journal tail until compaction, so a cold mount after a crash
still opens recently written volumes. A first mount with a clean journal read the device 76 times
with 8 volumes closed and 78 with 256.

Guests create no volumes yet: tests make them on the host (`Image::create_volume`). Creating one
from a manifest is TASK-136.3 and TASK-136.5.

### Base layers in Fxfs (TASK-136.3)

`StoreInfo` gains layers that are not root-store objects, a new format version. A base layer is read
through a `LayerObject` over its extents. Base layers:

- stay at the bottom of the tree, out of every compaction's merge, forced major ones included
  (`crates/fxfs/src/object_store/tree.rs`, `journal.rs:2336`). Tombstones already survive above
  kept layers: `major_iter` runs only when `layers_to_keep` is empty.
- count towards no metadata reservation.
- hold extents the allocator's table never holds, which behave as a frozen owner's: never freed,
  and shared already for reflinks. fsck accepts them.

The sandbox's allocator and root store never learn of the base.

### Publishing (TASK-136.4)

Infrastructure updates a repo in a base store, flushes its volume to layers, forks the store and
writes a manifest of the volume's `StoreInfo` and its layers' extents, so that no guest replays the
base's journal. The fork is the generation: it holds every extent as of the fork (doc-4), so the
base moves on without `snapshot_volume`. The flush comes first because a manifest can only name
layers, and a fork is as of the last commit.

Each generation holds every object of the earlier ones, unreachable ones included, so that a
sandbox's alternates can move to the latest and lose nothing.

A publisher needs no VM: `fxdax` runs Fxfs in process on an extent store
(`crates/fxdax/src/storage/image.rs:45`, `crates/fxdax/src/volume.rs:51`). The repository has no
Rust Git yet. From memory, unchecked, `gix` checks out only to real paths, so a publisher may read
trees with it and create files through the POSIX layer. It can then build `.git/index` from the
attributes Fxfs returns, stamped later than every file so that no entry is racy.

### Attaching (TASK-136.5)

An attach names a generation, pinned by an ordinary fork of the base's branch: existing collection
applies, and sandboxes on one generation share one pin. `READ` and `MAP` of an extent the branch
doesn't hold fall through to the attached forks, routed on the branch in the extent's ID, not by
scanning them. The extent cache is keyed by extent ID, so pages stay one copy.

This changes doc-4's rule that an extent is the guest's or inherited, in the VM attachment, not the
store core. Attachments are the host's, kept across snapshot, restore and reopening.

A guest names the extent IDs it reads, so everything reachable from an attached base is readable to
it. Tenancy rests on what the sandbox manager attaches. `make_public` and per-tenant sockets already
exist.

### Automount (TASK-136.6)

`d_automount` on a path lookup, combining with gyit's path-is-the-URL idea (`~/docs/projects/gat.md`).
As only the sandbox manager attaches, the lookup waits on an upcall through a guest agent.

### Hundreds mounted at once

A sandbox with hundreds of repos must not pay for those it isn't using:

- A cold mount opens only the volumes it mounts (built, TASK-136.2).
- An idle automount expires, as NFS referrals' do (from memory): its store closes and its writable
  layers stay, and its next lookup mounts it again (TASK-136.6).
- All volumes share one tree-cache budget (built, TASK-136.2), and the device's fall-through
  doesn't grow with attachments (TASK-136.5).

### Moving to a newer generation (TASK-136.7)

A mounted worktree's base doesn't change. `git fetch` moves a repo's objects to a newer generation
at the Git level, where objects are content-addressed and nothing can conflict:

1. A remote helper installed through `url.insteadOf` runs first, as Git has no pre-fetch hook (from
   memory). It asks the guest agent, which asks the sandbox manager to attach the newest generation.
2. The generation mounts read-only at a path of its own, which can automount, so that the path
   resolves after a restore.
3. `.git/objects/info/alternates` points at its objects. The fetch then transfers only what is newer
   than the generation.
4. `git pull` writes only what changed into the writable layers.

A refused attach leaves an ordinary fetch. Pulled changes stay in the writable layers, which grow
with them, and the first generation stays pinned.

An in-place swap would replace a volume's base layers under the live mount on a fast-forward, so
that those layers stop growing and old generations unpin. Part of it exists: `set_layers` already
installs layers under readers (`crates/fxfs/src/object_store/flush.rs:357`), and a follower's change
sets drop what the kernel caches and hold open files (doc-8), on read-only mounts. It would still
need stable object IDs across generations, object ID ranges that publisher and sandboxes don't
share, a record-level three-way check (`.git/index` and directory attributes conflict on nearly
every swap), writable mounts taking the follower's barrier, pins kept for extents the writable layers
still name, and a format change. Publishers updating one volume in place keep this open.

## Proof

On a Linux-sized checkout: mount time, first `git status`, first write, and memory per sandbox, and
the same with hundreds of repos mounted at once (TASK-136.1). There is no overlayfs comparison:
Ramon dropped it on 2026-10-31. Results go here.

## Open questions

- **`.git/index`.** Expected valid: object IDs, which are inode numbers, and stored times carry
  over, and Git ignores `st_dev` unless built with `USE_STDEV` (from memory, unchecked). The base
  needs the sandbox's uid, and an index refreshed after racy timestamps. TASK-136.4 tests it.
- **Metadata across VMs.** Only file data shares through DAX. Layer blocks are copied into each
  guest, which matters when `git status` stats a large tree. Two ways to share more, neither
  measured: warm a zygote before snapshotting it, which relies on Firecracker's private mapping of
  the memory file, unchecked here; and map base layers through the DAX window, as
  `LayerObject::as_slice` allows, if layers' block sizes permit.
- **Where attachments are kept**, durably, so that a stopped store reopens with them.
- **Pinned generations.** How long they stay while a sandbox store outlives its use of them.
- **Whether an in-place swap is worth building**, once the writable layers' growth under
  long-lived sandboxes is measured.
