---
id: doc-4
title: Virtio extent-store device specification
type: specification
created_date: '2026-10-13 22:18'
updated_date: '2026-10-18 11:15'
---
# The virtio extent-store device

Specification, 2026-10-13 (TASK-26.2, revised the same day). This is the only interface between the
guest's Fxfs and the host's extent store (doc-2). The guest's driver is TASK-26.5, and the VMM's
device is fxdax's Firecracker's (vendor/firecracker), first libkrun's (TASK-26.4; libkrun was
removed in TASK-40).

## Purpose

Efficient access to object storage from within a guest's kernel. Fxfs is the first client: it
stores its whole filesystem through the device, and anything else in the guest could use it the
same way, minting object IDs and keeping its own index of them.

It is a paravirtual device for an object store, in place of a block device, for two reasons:

- **No finite size.** A block device has one: a range of addresses to fill, and free space to manage
  inside it. An object store has none: it holds what is written, and its only limit is a quota.
- **The host's caches, shared.** The host keeps what VMs read in its memory and on its NVMe, and
  shares it between every VM on the host for every extent they have in common: forks of one VM,
  VMs of one base image, or files that happen to be the same extents. A guest maps shared pages
  straight from the host's memory (DAX), and reads miss to the host's NVMe before object storage.
  A block device's addresses mean nothing outside one VM, so it can share none of this.

## The model

The store holds **extents**: immutable pieces of data, each named by an ID the guest mints. There
are no addresses, no free space and no device size.

- **IDs.** An extent ID is 128 bits: the branch's 64-bit ID, which the host assigns when it creates
  or forks the filesystem, and a 64-bit sequence number that the guest's Fxfs advances. Minting
  needs no request, and IDs never collide across forks or hosts.
- **Write-once.** The guest writes an extent, in pieces if it likes, until the next barrier. The
  barrier seals it, and it never changes again. Writing a sealed extent is an error.
- **Freeing.** The guest frees an extent once nothing in its filesystem refers to it. The host
  deletes it once every branch that has it has freed it.
- **The root.** Each branch has one mutable record: the root, which names where its filesystem's
  metadata starts. A commit can set it.
- **Capacity.** Capacity is a quota the host reports. The guest counts its extents against it.

### Why

doc-4's first version gave each filesystem a sparse 2^48-byte address space. It kept Fxfs as it
was, but it kept a block device's worldview with it:

- offsets, free space, best-fit allocation and trimming, over a space mostly never written;
- a device size standing in for capacity;
- a host that has to reconstruct what to pack, share, fork and delete from ranges of addresses.

Extents are what object storage, a shared page cache, forks and garbage collection all work in, so
the device speaks them directly.

### Decisions

Settled in a design review on 2026-10-13:

| Decision | Choice | Rejected |
|---|---|---|
| What the guest names | Extents, any size: one per extent Fxfs writes | Fixed-size chunks (3FS, JuiceFS); guest-built log segments, which need a cleaner in the guest |
| How extents become object-store objects | **Hybrid:** small extents are packed, in commit order, into segment objects of about 16-64 MiB; extents of 4 MiB and up are objects of their own | One object per extent, or per block of one (JuiceFS), where every small file is at least one PUT; packing everything, which makes cleaning copy large files |
| Mutability | Write-once, sealed by the next barrier | Appendable extents; extents the host versions |
| IDs | Branch and sequence, 128 bits | Host-granted 64-bit ranges; content hashes |
| Freeing | By the guest, of whole extents: dead parts stay stored until the extent dies or Fxfs rewrites the file | Freeing byte ranges; the guest reporting live ranges for the host to trim |
| Fxfs's format | Extents only: fxdax's image files become a local extent store | Fxfs keeping its block-device mode too; a separate Fxfs for the guest |

## Device

- **Device ID:** `0x4658` ("FX"), outside the virtio IDs OASIS assigns. The transport is virtio 1.x,
  over MMIO in fxdax's Firecracker.
- **Byte order:** every field is little-endian.

### Features

| Bit | Name | Meaning |
|---:|---|---|
| 0 | `VIRTIO_EXTENT_F_DAX` | Shared memory region 0 is a DAX window, and `MAP`/`UNMAP` work. |
| 1 | `VIRTIO_EXTENT_F_DAX_WRITE` | `MAP` maps open extents writably. Needs `DAX`. |
| 2 | `VIRTIO_EXTENT_F_WRITE_COMMIT` | `WRITE` takes `WRITE_COMMIT`: a commit follows the write. |
| 3 | `VIRTIO_EXTENT_F_RESIZE` | The configuration holds `capacity` and `asked`, which change while the device runs (Resizing); the guest answers a shrink with `RESIZED`. |
| 4 | `VIRTIO_EXTENT_F_FREEZE` | `FREEZE` works, and `INFO` says the `branch` (Snapshots). |
| 5 | `VIRTIO_EXTENT_F_READ_ONLY` | The device follows a store another VM writes, read-only (Followers): `ADVANCE`, `LET_GO`, `PIN` and `UNPIN` work, and the configuration holds `committed`, which changes while the device runs. Excludes `DAX_WRITE`, `WRITE_COMMIT` and `RESIZE`. |
| 32 | `VIRTIO_F_VERSION_1` | Required. |

### Configuration space

```c
struct virtio_extent_config {
	u8 tag[36];               /* the filesystem's name, NUL-padded: the mount's source */
	le32 num_request_queues;  /* at least 1 */
	le32 block_size;          /* 4096: the alignment of offsets and lengths */
	le32 map_alignment;       /* the host's page size: the alignment of MAP and UNMAP */
	le32 max_transfer;        /* the most bytes one READ or WRITE moves */
	le32 max_segments;        /* the most data descriptors one request carries */
	le32 max_root;            /* the most bytes the root holds: at least 512 */
	le64 branch;              /* the branch's ID: the first half of every extent the guest mints */
	le64 max_extent;          /* the most bytes one extent holds: at least 64 MiB */
	le64 capacity;            /* F_RESIZE: the store's capacity, as INFO's */
	le64 asked;               /* F_RESIZE: the shrink the guest is asked to take, or 0 */
	le64 committed;           /* F_READ_ONLY: the epoch of the writer's latest commit */
};
```

The configuration never changes while the device runs, but for `capacity` and `asked`, with
`F_RESIZE`, and `committed`, with `F_READ_ONLY`.

### Resizing

*Added 2026-10-17 (TASK-37).* The host resizes a store while its guest runs (`fxdaxctl
resize`, through the extent daemon). It grows it at once, sets `capacity`, and raises a
configuration change: the guest's filesystem takes the larger size, its device's block count and
its allocator's budget, while mounted. A shrink needs the guest, whose filesystem may have
reserved more than the store holds: the host sets `asked` and raises a configuration change; the
guest shrinks if its filesystem holds less than `asked`, with room left for its transactions
(16 MiB, in fxdax's Fxfs), and answers with `RESIZED`, whether it took it; the store shrinks only
then, never below what its extents hold. The host waits 10 s for the answer. A store no VM has
open grows, and does not shrink.

### Virtqueues

Queues `0` to `num_request_queues - 1` are request queues, interchangeable. A driver may use one per
CPU. Each request is one descriptor chain:

1. the header, which the device reads;
2. the request's arguments, which the device reads;
3. for `WRITE`, and for a `COMMIT` that sets the root, the data, which the device reads;
4. for `READ` and `ROOT`, the buffer for the data, which the device writes;
5. the response, which the device writes.

The driver may have as many requests in flight as the queues hold.

### Shared memory

With `VIRTIO_EXTENT_F_DAX`, shared memory region 0 is the DAX window: guest-physical memory whose
contents `MAP` sets. The transport gives its size, as virtio-fs's does.

## Requests

```c
struct virtio_extent_id {
	le64 branch;
	le64 sequence;
};

struct virtio_extent_req {
	le32 type;
	le32 flags;
	le64 unique;          /* the driver's own, for tracing */
};

struct virtio_extent_resp {
	le32 status;          /* 0, or an errno */
	le32 reserved;
};
```

| Type | Name | Arguments | Data |
|---:|---|---|---|
| 1 | `READ` | `struct virtio_extent_id id; le64 offset; le32 len; le32 reserved;` | device writes `len` bytes |
| 2 | `WRITE` | `struct virtio_extent_id id; le64 offset; le32 len; le32 reserved;` | device reads `len` bytes |
| 3 | `FREE` | `struct virtio_extent_id id;` | none |
| 4 | `MAP` | `struct virtio_extent_id id; le64 offset; le64 len; le64 window_offset;` | none |
| 5 | `UNMAP` | `le64 window_offset; le64 len;` | none |
| 6 | `COMMIT` | `le32 root_len; le32 reserved;` | device reads `root_len` bytes: the new root, if `COMMIT_ROOT` |
| 7 | `ROOT` | none | device writes `max_root` bytes, and `struct virtio_extent_root_len` |
| 8 | `INFO` | none | device writes `struct virtio_extent_info` |
| 9 | `RESIZED` | `le64 capacity;` (the `asked` it answers); flag `RESIZED_TOOK` (bit 0): it took it | none |
| 10 | `FREEZE` | none; flag `FREEZE_FROZEN` (bit 0): frozen, else thawed | none |
| 11 | `ADVANCE` | none | device writes `struct virtio_extent_epoch` |
| 12 | `LET_GO` | none | none |
| 13 | `PIN` | `struct virtio_extent_id id;` | none |
| 14 | `UNPIN` | `struct virtio_extent_id id;` | none |

### `WRITE`

Writes `len` bytes at `offset` in extent `id`. Both are multiples of `block_size`, and the end is
within `max_extent`.

- The extent must be the guest's own (its branch is `branch`), and open. The first write creates
  it.
- An extent's length is the end of its furthest write. Blocks within it that were never written
  read as zeros.
- The data is in the store, and visible to reads and mappings, when the request completes. It is
  durable as part of its epoch (see Ordering).
- With `WRITE_BARRIER` (bit 0), the write follows a barrier, as if a `COMMIT` were submitted just
  before it: Fxfs's `PRE_BARRIER` writes.
- With `WRITE_COMMIT` (bit 1, which needs `F_WRITE_COMMIT`), a barrier follows the write, as if a
  `COMMIT` were submitted once it completes, durable with `WRITE_DURABLE` (bit 2), as with
  `COMMIT_DURABLE`; the request completes once that commit does. It seals the extent written too:
  Fxfs's `POST_BARRIER` writes, the last of a journal flush that ends a sync, one request rather
  than a `WRITE` and a `COMMIT`, each a round trip to the host.

**Sealing.** An extent is open from its first write, or its first writable mapping (`MAP`), until
the first barrier submitted after a write or a store to it completes. That barrier seals it. Every
write to an extent must therefore complete before the next barrier is submitted: Fxfs writes an
extent's data, then commits the transaction that refers to it, so this holds.

### `READ`

Reads `len` bytes at `offset` in extent `id`. Both are multiples of `block_size`, and the range lies
within the extent's length. The extent is the guest's, or one its branch inherited from the branch
it was forked from. A `READ` sees every `WRITE`, and every store through a writable mapping,
completed before it was submitted.

Reading an extent the store doesn't hold, one never written or one freed, fails with `ENOENT`. The
guest's Fxfs reads such an extent as zeros where it may never have been written: preallocated
extents, and blocks of them not yet written (TASK-26.3).

### `FREE`

Frees extent `id`: nothing in the guest's filesystem refers to it any more. It need not be sealed.
Freeing an extent the store doesn't hold succeeds: after a crash, the guest frees again what it may
have freed before.

- Afterwards, reading or mapping it fails.
- The host deletes it once the `FREE`'s epoch is durable and no other branch has it.
- The guest must not free an extent while requests on it are in flight, and never mints the same ID
  twice.

### `MAP` and `UNMAP`

`MAP` maps `len` bytes at `offset` in extent `id` into the window at `window_offset`. All three are
multiples of `map_alignment`, and the range lies within the window.

- Loads see the extent's contents. What of a read-only mapping lies past the extent reads as zeros,
  as blocks within it never written do: blocks a write allocated, which a barrier sealed unwritten,
  and which the write then moves elsewhere, which the guest may map as it reads them meanwhile.
- The mapping is read-only, unless `MAP_WRITE` (bit 0) is set, which needs `DAX_WRITE`. Stores to a
  read-only mapping fault in the guest.
- A read-only mapping of an open extent sees its contents as they are written and stored to, until
  the barrier that seals it: a guest may take an extent for sealed while that barrier is in flight.
- A writable mapping is of an extent of the guest's branch that is open, or that does not exist yet,
  which it opens as a first write would: an extent the guest mints for data it stores through the
  window. The extent is then at least as long as the furthest end of a writable mapping or a write
  to it, up to `max_extent`, and bytes neither written nor stored to read as zeros.
- Stores through a writable mapping belong to the current epoch. The barrier that seals the extent
  makes its writable mappings read-only; they keep its contents.
- `MAP_WRITE` of a sealed extent, or of another branch's, fails with `EPERM`.
- A `MAP` replaces whatever the window held there.

`UNMAP` makes `len` bytes of the window at `window_offset` inaccessible.

Every VM on a host that maps the same extent maps the same host pages, which is safe because
extents never change once sealed.

### `COMMIT`

A barrier: every request completed before the `COMMIT` was submitted, and every store through a
writable mapping before then, is ordered before every request submitted after it completes. It
seals every extent written before it.

- **With `COMMIT_ROOT`** (bit 1), the commit also sets the branch's root to the `root_len` bytes it
  carries (at most `max_root`), ordered with everything before it.
- **Without `COMMIT_DURABLE`,** the commit completes once its order is fixed. It does not wait for
  durability: doc-2's requirement 2, under which fsync keeps its ordering but a crash may lose a
  suffix.
- **With `COMMIT_DURABLE`** (bit 0), it completes once everything before it is durable: on local
  disk in milestone 1, uploaded once object storage arrives (milestone 3). Unmount and `sync` use
  it.

### `ROOT`

Reads the branch's root: the guest's filesystem starts there.

```c
struct virtio_extent_root_len {
	le32 len;          /* bytes of the root, 0 if the branch has none */
	le32 reserved;
};
```

The device writes the root into the first `len` bytes of the `max_root`-byte buffer. A new
filesystem's root is empty: formatting sets it.

### `INFO`

```c
struct virtio_extent_info {
	le64 capacity;     /* bytes the store will hold for this branch */
	le64 used;         /* bytes of extents it holds for it */
	le64 available;    /* bytes it can still take */
	le64 next_sequence; /* one more than the highest sequence number the branch has written */
	le64 sequence_floor; /* the lowest sequence number the guest may mint from */
	le64 branch;       /* F_FREEZE: the branch the guest mints extents of */
};
```

A driver without `F_FREEZE` gives room for the fields before `branch`, which the device writes
alone. With it, the guest takes its branch from `INFO`, not the configuration, which the driver
read as it bound the device: a VM restored from a snapshot has a branch of its own (Snapshots).

For `statfs`, and for the guest's own count of what it may write.

`next_sequence` counts every extent the branch wrote, freed or not, and recovers with the store
after a crash. A crash may leave extents written whose references the guest's journal lost:
orphans. After replay, the guest mints from the largest of its own next sequence number,
`next_sequence` and `sequence_floor`, and frees the extents between its own and `next_sequence`
that nothing refers to.

`sequence_floor` is 0 unless the store keeps its branch in object storage, where it is its
writer's range's start (doc-7, Fencing): IDs below it another writer may have minted. No extent
the store holds need reach it, so the guest looks for orphans below `next_sequence` only: between
its own count and the floor there may be 2^40 numbers.

### Errors

| Status | When |
|---|---|
| `0` | Success. |
| `EPERM` (1) | A write, or a writable `MAP`, to a sealed extent, or to another branch's. |
| `ENOENT` (2) | A `READ` or `MAP` of an extent that does not exist, or was freed. |
| `EIO` (5) | The store failed to read or write. |
| `EINVAL` (22) | An unknown type or flag, a misaligned or out-of-range offset, length or window offset, or a feature's flag the device does not offer. |
| `ENOSPC` (28) | The store cannot hold more for this branch. |
| `EROFS` (30) | The store stopped taking writes for this branch: another host took its lease (milestone 4), or the host is shutting it down. Reads still work. With `F_READ_ONLY`, every `WRITE`, `FREE`, `COMMIT`, writable `MAP` and `RESIZED`. |
| `ESTALE` (116) | `F_READ_ONLY`: the follower was cut off (Followers). |

With `F_READ_ONLY`, a `READ` or `MAP` of an extent the follower's view doesn't hold fails with
`EIO`, not `ENOENT`, and the host logs it: the guest's follower reads only what its commit holds,
so a read outside it is a bug, which fails loudly (doc-8).

## Followers

*Added 2026-10-18 (TASK-42.3, doc-8).* A VM on the writer's host follows a store another VM
writes, read-only, with a device offering `F_READ_ONLY`: its guest mounts the filesystem as of a
commit of the writer's, and applies the writer's commits after it as they come, while the writer
goes on and never waits for it. The host serves it the same extents, from the same memory.

- **Its view.** The device reads the store as of a commit, its **epoch**: what that commit holds,
  its sealed extents and root, and nothing the writer wrote after it. The writer's frees after it
  are kept while the view may read them. `ROOT` is the view's commit's root. A device starts as of
  the writer's latest commit, or the store's last, if no VM writes it.
- **Commits.** Each commit of the writer's after the view's sets `committed` to its epoch and
  raises a configuration change (the latest of a run of commits, at least once).
- **`ADVANCE`** moves the view to the writer's latest commit, and returns its epoch:

  ```c
  struct virtio_extent_epoch {
  	le64 epoch;
  	le64 reserved;
  };
  ```

  The view still holds what the commits between held, as the guest's follower reads them while
  it applies those commits, and its readers may be reading them still, until `LET_GO`.
- **`LET_GO`** lets go of what the commits before the view's held: the writer's frees up to its
  commit go, but for what it pins.
- **`PIN`** keeps extent `id`, which the view holds, whatever commit it advances to, until
  `UNPIN`: what the guest holds open that the writer deletes (doc-8's graveyard).
- **Cut off.** A follower too far behind (a lag in time, or in bytes of frees it keeps), or whose
  writer went with writes uncommitted, is cut off: its `READ`s, `MAP`s, `ADVANCE`s and `PIN`s
  fail with `ESTALE` until its guest remounts, with the device reset.

The host serves a follower's store to its writer as it would without followers: a writer that
went with nothing open (its last commit all it wrote) leaves its followers following, and the
next writer takes the store over; one that went with writes uncommitted cuts them off, and no VM
opens the store again until they are gone.

## Ordering and durability

Requests on all queues are applied in any order, except across barriers. `COMMIT`s, and the
barriers of `WRITE_BARRIER`s and `WRITE_COMMIT`s, divide the requests into **epochs**:

- A request completed before a barrier was submitted belongs to an epoch before it.
- A request submitted after a barrier completed belongs to an epoch after it.
- A request in flight while a barrier is may fall on either side.
- Stores through writable mappings belong to the epoch in which they were made.

After a crash of the guest, the VMM or the host, the store holds:

- everything from the epochs before some barrier B, including the last root set before it;
- any subset of epoch B's writes and frees, each block written whole or not at all;
- nothing after it.

That is how disks behave with barriers, and how Fxfs's journal recovers: checksums find where a torn
journal ends. With `COMMIT_DURABLE`, B is at least that commit. How far B can lag the latest commit
is the host's lag, bounded by its configuration (doc-2, requirement 11).

## Branches and forks

Each VM's filesystem is a branch, with its own ID and root.

- **Forking** a branch makes a child with a new ID, a copy of the parent's root, and every extent
  the parent had at the fork. It takes O(1): the host records the parent and the fork point.
- **After a fork,** each branch mints extents under its own ID. A branch that frees an inherited
  extent drops only its own claim, and the host deletes the extent once every branch that has it has
  freed it.
- **Reads** of inherited extents, and mappings of them, are as of the fork. Extents never change, so
  this needs no copying.

The guest sees none of this except a different `branch` in its configuration after a fork.

## Snapshots

A VMM snapshots a VM with the device, and restores the snapshot, as many times as it likes, each
restored VM with a store of its own (fxdax's TASK-39.1): a zygote's children, suspend-to-RAM, a
VM moved to another host. Extent IDs carry their branch, and the restored guest has the branch it
had in its memory, so the snapshot is taken at a point nothing is open:

- **Freezing.** The guest's filesystem freezes first (fsfreeze): it commits durably, sealing what
  is open, holds its background work, and sends no `WRITE`, `FREE` or `COMMIT` until it thaws;
  `READ`, `MAP` of sealed extents and `UNMAP` go on. It tells the device with `FREEZE` and
  `FREEZE_FROZEN`. A VMM refuses to snapshot a device whose guest isn't frozen.
- **The snapshot** holds the device's state, its window's place in guest memory and what each
  mapping of the window maps (an extent's bytes, or zeros), and a point of the store: the host
  forks the store, as of its last commit, which with nothing open is all the guest wrote.
- **A restore** opens a store of its own, forked from the snapshot's point (or the store the VM
  had, if nothing else changed it), and maps the window as the snapshot had it, at the same guest
  physical address. Its configuration's `branch` is the new store's.
- **Thawing,** the guest sends `FREEZE` without the flag, and takes its `branch` from `INFO`, which
  it mints extents of from then on: its journal continues in new extents of it before anything
  else commits.

A guest that never freezes never sees a snapshot taken.

## The host's storage

This section describes how the host stores extents. It is not part of the protocol.

- **Packing.** Sealed extents smaller than 4 MiB are packed, in commit order, into **segments** of
  about 16-64 MiB. Extents of 4 MiB and more are stored on their own, as objects of their own.
- **The index.** An index maps each extent ID to its segment and offset, or to its own object. Each
  upload adds to it, with periodic checkpoints.
- **Cleaning.** The host rewrites segments that are mostly freed extents, copying the live small
  extents into new segments. Large extents are deleted whole and never copied.
- **Milestone 1** keeps segments, large extents and the index in files on local disk. Milestone 3
  uploads them to object storage, in the layout doc-2 describes, and keeps local disk as a cache.
- **Caches.** The host's memory and NVMe cache extents by ID: pages in memory, which VMs map through
  their DAX windows, and extents on NVMe, which misses in memory read from before object storage.
  Every VM, fork and snapshot on the host that reads the same extent shares both.

## What Fxfs becomes

One on-disk format: extents only (TASK-26.3). The same Fxfs runs on the host, for fxdax's virtio-fs
path, whose image files become a local extent store, and in the guest.

| Fxfs | Becomes |
|---|---|
| Device offsets in extent records | `(extent ID, offset)` |
| The allocator: a free-space tree, best-fit, reservations against the device's size | An ID minter (the branch and a sequence number stored in its info), reference counts per extent, and reservations against `INFO`'s capacity |
| Trimming | `FREE`, sent when an extent's last reference is gone, the device has been flushed past that, and the journal records the flush (replay must not verify checksums of writes to extents freed) |
| Superblocks at fixed offsets | Written to new extents, which the root names |
| The journal's preallocated, appended extents | New extents for each journal flush |
| Overwrite mode for preallocated files | Copy-on-write |
| `Device::flush` | `COMMIT` |
| `PRE_BARRIER` writes | `WRITE_BARRIER` |
| `POST_BARRIER` writes (fxdax: a sync's journal flush, whose barrier is its flush) | `WRITE_COMMIT` |
| Closing | `COMMIT_DURABLE` |
| fxdax's snapshots (frozen data owners) | References to the same extents |

Fxfs runs with barriers enabled.
