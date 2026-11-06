---
id: doc-10
title: 'Shared volumes: Fxfs volumes over read-only bases'
type: specification
created_date: '2026-10-30 09:08'
updated_date: '2026-11-06 06:43'
---
# Shared volumes: Fxfs volumes over read-only bases

Status, 2026-10-31. Built: the guest's mounts of volumes (TASK-136.2), volumes over base layers in
Fxfs (TASK-136.3), attaching a base to a store (TASK-136.5), and publishing from a sandbox and
importing in another (TASK-136.4), which the test `fxfs-guest-shared-volumes` runs with Git. Not
built: the sandbox manager's part, lookups that attach (TASK-136.6), and moving to a newer generation
(TASK-136.7). Work: TASK-136.
It sits on top of the filesystem and is separate from TASK-96, the abstract extent store split. Idea
note: `~/docs/ideas/fxfs-native-overlay-for-shared-repos.md`.

The feature is generic: a shared volume is a read-only base that any sandbox can attach and write
over, privately. Git is its first use, not part of it: many users clone the same repos over and over
and keep them up to date, so a sandbox keeps a checkout current in a volume, and other sandboxes get
it with a writable layer of their own, mounted at a subpath in the VFS, not as the root, with
overlay semantics from Fxfs primitives. Nothing in Fxfs, the store or the daemon knows of Git.

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
| What the feature is | Generic shared volumes, which Git happens to use | A feature that knows of Git |
| Who keeps a shared volume current | A sandbox, running ordinary commands: Git, for repos | A publisher on the host, with Git in process |

Proposals, not yet confirmed: new mounts taking the latest generation, and untouched repos advancing at their next mount.

## Why not bake repos into a base store

A store fork already makes every volume private, with no `snapshot_volume`, and needs no store or
device change. But the base's whole size lands on each cold mount:

- Every mount opens every volume (`crates/fxfs/src/object_store/object_manager.rs:261`), loading
  each layer's seek table and bloom filter. (Since TASK-136.2, not with `Options::lazy_volumes`,
  below.)
- Every mount scans the whole allocator tree into memory
  (`crates/fxfs/src/object_store/allocator.rs:1053`).
- Each volume's layers reserve 1.5 times their size (`object_store.rs:2168`), and each volume's
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
  across volumes. An unmount with others staying drops its ranges and leaves the private pages'
  budget worker running for them.
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

Guests make volumes only from manifests (`FXFS_IOC_IMPORT`, below). Empty ones are made on the host
(`Image::create_volume`).

### Base layers in Fxfs (TASK-136.3)

Built. `StoreInfoV61`, format 61, lists `base_layers`, each a layer's size and extents, and
`base_branches`, the branches of every extent the base names. A base layer is read through
`BaseLayerObject`, a `LayerObject` over its extents (`crates/fxfs/src/object_store/base_layer.rs`).
Base layers:

- stay at the bottom of the tree, out of every compaction's merge, forced major ones included
  (`tree::layers_to_merge`). Tombstones survive above them: `major_iter` runs only when
  `layers_to_keep` is empty.
- count towards no metadata reservation: nothing could rewrite them. They are of the latest
  version, as their manifest is, or the store doesn't open (`FxfsError::InvalidVersion`, TASK-141.1).
- hold extents the allocator's table never holds. The allocator notes `base_branches` in memory as a
  store opens, so that its extents are never deallocated, always shared and never referenced, and
  fsck leaves them out. No allocator record or mutation changed.

The manifest is a `StoreInfo` with no layers of its own: `RootVolume::publish_volume` compacts a
volume to one layer and describes it, and `new_volume_over_base` writes that as a new store's
information, in one transaction. So a snapshot of such a volume, and a publish of one, work as for
any volume, a publish putting its layers above those it has. An import is refused if a base branch
is the device's own or one its allocator holds extents of, as it would lose count of those. A
manifest is another filesystem's, so it is checked: order, alignment, bounds, version and size, at
most half of what a `StoreInfo` may take.

The sandbox's allocator and root store never learn of the base. Limits:

- Opening a volume over a base read the fork 3 times with 3 files and 4 with 503: a layer's two ends
  and its bloom filter, which grows with the layer, as for any volume. An import reads nothing more
  than that open does.
- Layers read this way skip Fxfs's per-block checksums, which stay in the publisher's extent records.
- A bloom filter counts only in a layer of the latest format, so bases of an older one lose it.
- A file preallocated in overwrite mode can't be overwritten in a base, as in a snapshot.

### Publishing (TASK-136.4)

Built. A sandbox keeps the volume current with ordinary commands, and the host forks its store and
keeps the manifest, so that no guest replays its journal. The fork is the generation: it holds every
extent as of the fork (doc-4), so the sandbox moves on with no `snapshot_volume`. The three ioctls
are in `guest/linux/fs/fxfs/vfs.h`. In order:

1. `syncfs`, once the commands are done.
2. `FIFREEZE` on the volume, which stops its writers.
3. `FXFS_IOC_FLUSH` on the volume, which puts all it holds in one layer. A manifest can only name
   layers. This takes no VFS write, so it runs on a frozen superblock, but it needs the filesystem
   not frozen, so some other volume stays mounted and thawed: the default one will do.
4. `FIFREEZE` on every other mounted volume, which freezes the filesystem. The host's fork needs
   this whatever is published: it refuses (`EBUSY`) with anything open on the device.
5. `FXFS_IOC_MANIFEST` on the volume. The filesystem must be frozen (`EINVAL` otherwise), so no
   compaction frees what the manifest names before the fork holds it, with no hold on the store to
   release. It fails with `EBUSY` if the journal holds anything of the volume: thaw and repeat.
6. The host forks the store, and marks the fork public if it is (`MakePublic` with `generation`).
7. `FITHAW` on them all.

A consumer asks the sandbox manager to attach the generation, then `FXFS_IOC_IMPORT` with the
manifest makes a volume of any free name, which mounts as any other. The volume stays the consumer's
after reboots, and each generation is one more import.

For a consumer's first `git status` to lstat every tracked file and hash none, shown on a 200-file
repo in `fxfs-guest-shared-volumes`, with and without DAX:

- **The files belong to whoever consumers run as, from the clone on.** The index records a file's
  owner and ctime, so a `chown` after it makes every file hash, which the test shows. It is the
  file's owner that counts, not who runs Git.
- **The index is written again after the last change, in a later second.** Git doesn't trust a file
  whose mtime is not older than the index. The publisher waits for the second to pass, then runs
  `git update-index --refresh` and `git update-index --force-write-index`. With this left out, the
  files the last update wrote were opened, and no others.
- **Nothing else is needed.** Object IDs, stored times and sizes carry over, and Fedora 43's Git
  ignores `st_dev`, which differs. A mount path may differ too, though alternates want a stable one
  (TASK-136.7).

What running the publisher in a sandbox asks of it:

- **Consumers trust it.** A consumer's kernel parses layers another guest wrote, and a volume carries
  what a clone leaves behind, such as `.git/hooks` and `.git/config`. A hostile repo that exploits
  Git in its publisher reaches every sandbox attaching that volume, and fsck doesn't change that.
  So one publisher per public repo, at most one per tenant for private ones, and the host treats a
  manifest as opaque, never parsing it.
- **A long-lived store, one per volume.** Inherited extents keep the branch that minted them, so a
  chain of throwaway forks spreads a generation over a growing list of branches to route. Generations
  fork sideways off one store, which `MakePublic` with `generation` leaves writable. A generation
  pins that whole store, so scratch files stay out of it.
- **The Fxfs format is a contract between VMs.** `EARLIEST_SUPPORTED_VERSION` is `LATEST_VERSION`,
  61: nothing is migrated (AGENTS.md), so a kernel takes a manifest, and opens base layers, of its own
  version alone, older and newer refused (TASK-141.1). Every format change strands every generation,
  which a kernel of the new version must publish again; until then publishers and the VMs over their
  bases stay on one kernel.
- **The maintenance scripts keep Git's rules,** which the feature can't: no pruning, or alternates
  lose objects (`gc.auto` 0); no full `git repack -a -d`, which rewrites packs so generations stop
  sharing extents; the owner and index rules above; a stable mount path.

Each generation holds every object of the earlier ones, unreachable ones included, so that a
sandbox's alternates can move to the latest and lose nothing.

### Attaching (TASK-136.5)

Built. `AttachBase` names a generation, pinned by an ordinary fork of the base's branch: existing
collection applies, and sandboxes on one generation share one pin, one `ExtentStore` the daemon
opens the first time any of them reads through to it. `READ` and `MAP` of an extent of a
branch in the fork's ancestry, and not the sandbox's own, go to it, by a lookup of the branch in the
extent's ID (`Bases`, `crates/fxdax/src/extents.rs`). The extent cache is keyed by extent ID, so
pages stay one copy. However many are attached, each costs a line and map entries until it is read.

This changes doc-4's rule that an extent is the guest's or inherited, in the VM attachment, not the
store core: `crates/extent_store` didn't change. The record is a file beside the store, in its
directory (`attached-bases`), which a fork copies before its commit, so that forks and
restored VMs keep it, and which goes with the store. A guest has no request that touches it.

A guest names the extent IDs it reads, so everything reachable from an attached base is readable to
it. Tenancy rests on what is attached: a store that isn't public attaches only to a store in its
tenant's directory (`EACCES` otherwise). A generation made public with `MakePublic`'s `generation`
is marked alone, so its store stays writable, and its pages are the default arena's through it.

Limits:

- A generation is the caller's to keep. `DeleteStore` refuses one a VM has open, but a stopped store
  doesn't hold it, and then reads `ENOENT` of it, and opens all the same.
- The record is local to the host. A store taken from object storage on another host has nothing
  attached, and a generation open on several hosts takes its branch from one another; reads of it
  are expected to go on, unchecked.
- A guest maps a base's extents by ranges, not regions, as the allocator has no size for them
  (`extent_size` 0), which DAX takes as unknown.

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
installs layers under readers (`crates/fxfs/src/object_store/flush.rs:362`), and a follower's change
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

- **Metadata across VMs.** Only file data shares through DAX. Layer blocks are copied into each
  guest, which matters when `git status` stats a large tree. Two ways to share more, neither
  measured: warm a zygote before snapshotting it, which relies on Firecracker's private mapping of
  the memory file, unchecked here; and map base layers through the DAX window, as
  `LayerObject::as_slice` allows, if layers' block sizes permit.
- **Pinned generations.** How long they stay while a sandbox store outlives its use of them, and
  who keeps a stopped store's from going.
- **Attachments across hosts.** The record is local, and a generation's branch has one writer's
  lease at a time.
- **Whether an in-place swap is worth building**, once the writable layers' growth under
  long-lived sandboxes is measured.
