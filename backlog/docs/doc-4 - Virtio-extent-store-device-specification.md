---
id: doc-4
title: Virtio extent-store device specification
type: specification
created_date: '2026-10-13 22:18'
updated_date: '2026-10-13 22:50'
---
# The virtio extent-store device

Specification, 2026-10-13 (TASK-26.2, revised the same day). This is the only interface between the
guest's Fxfs and the host's extent store (doc-2). The guest's driver is TASK-26.5, and libkrun's
device is TASK-26.4.

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
  over MMIO in libkrun.
- **Byte order:** every field is little-endian.

### Features

| Bit | Name | Meaning |
|---:|---|---|
| 0 | `VIRTIO_EXTENT_F_DAX` | Shared memory region 0 is a DAX window, and `MAP`/`UNMAP` work. |
| 1 | `VIRTIO_EXTENT_F_DAX_WRITE` | `MAP` maps open extents writably. Needs `DAX`. |
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
};
```

The configuration never changes while the device runs.

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

**Sealing.** An extent is open from its first write until the first barrier submitted after a write
to it completes. That barrier seals it. Every write to an extent must therefore complete before the
next barrier is submitted: Fxfs writes an extent's data, then commits the transaction that refers to
it, so this holds.

### `READ`

Reads `len` bytes at `offset` in extent `id`. Both are multiples of `block_size`, and the range lies
within the extent's length. The extent is the guest's, or one its branch inherited from the branch
it was forked from. A `READ` sees every `WRITE`, and every store through a writable mapping,
completed before it was submitted.

### `FREE`

Frees extent `id`: nothing in the guest's filesystem refers to it any more. It need not be sealed.

- Afterwards, reading or mapping it fails.
- The host deletes it once the `FREE`'s epoch is durable and no other branch has it.
- The guest must not free an extent while requests on it are in flight, and never mints the same ID
  twice.

### `MAP` and `UNMAP`

`MAP` maps `len` bytes at `offset` in extent `id` into the window at `window_offset`. All three are
multiples of `map_alignment`, and the range lies within the window and the extent.

- Loads see the extent's contents.
- The mapping is read-only, unless `MAP_WRITE` (bit 0) is set and the extent is the guest's and
  open, which needs `DAX_WRITE`. Stores to a read-only mapping fault in the guest.
- Stores through a writable mapping belong to the current epoch. The extent's sealing makes the
  mapping read-only.
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
};
```

For `statfs`, and for the guest's own count of what it may write.

### Errors

| Status | When |
|---|---|
| `0` | Success. |
| `EPERM` (1) | A write to a sealed extent, or to another branch's. |
| `ENOENT` (2) | An extent that does not exist, or was freed. |
| `EIO` (5) | The store failed to read or write. |
| `EINVAL` (22) | An unknown type or flag, a misaligned or out-of-range offset, length or window offset, or a feature's flag the device does not offer. |
| `ENOSPC` (28) | The store cannot hold more for this branch. |
| `EROFS` (30) | The store stopped taking writes for this branch: another host took its lease (milestone 4), or the host is shutting it down. Reads still work. |

## Ordering and durability

Requests on all queues are applied in any order, except across barriers. `COMMIT`s and
`WRITE_BARRIER`s divide the requests into **epochs**:

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
| Trimming | `FREE`, sent when an extent's last reference is gone and committed |
| Superblocks at fixed offsets | Written to new extents, which the root names |
| The journal's preallocated, appended extents | New extents for each journal flush |
| Overwrite mode for preallocated files | Copy-on-write |
| `Device::flush` | `COMMIT` |
| `PRE_BARRIER` writes | `WRITE_BARRIER` |
| Closing | `COMMIT_DURABLE` |
| fxdax's snapshots (frozen data owners) | References to the same extents |

Fxfs runs with barriers enabled.
