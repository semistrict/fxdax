---
id: doc-4
title: Virtio extent-store device specification
type: specification
created_date: '2026-10-13 22:18'
updated_date: '2026-10-13 22:18'
---
# The virtio extent-store device

Specification, 2026-10-13 (TASK-26.2). This is the only interface between the guest's Fxfs and
the host's extent store (doc-2). The guest's driver is TASK-26.5, and libkrun's device is
TASK-26.4.

## Decisions

### Storage is a sparse address space

The device gives each filesystem a sparse address space of `address_space` bytes (2^48 by
default).

- Storage is used only where the guest has written, and released where it frees. So it grows and
  shrinks with what the files hold, and the space has no size to fill.
- The host keeps the space in **extents**: aligned 16 MiB pieces. Each address is an extent and
  an offset (`extent = address >> 24`). Extents are the unit of the host's page cache, of forks,
  and of object-storage segments, but they never appear in the protocol.
- The guest allocates addresses itself, with Fxfs's allocator, and needs no request to do so.

Why:

- **Latency.** Fxfs allocates on every copy-on-write write. A host round trip per allocation
  would bring back the latency the design removes, so allocation has to be the guest's.
- **Fewer changes to Fxfs.** Fxfs records a 64-bit device offset in every extent record, places
  its superblocks at fixed offsets, and allocates and frees at block granularity, including
  partial frees. An address space keeps all of that as it is.

Rejected: extents that are each one Fxfs allocation, with an ID from a host-granted range, and
records holding `(id, offset)`. The host would know each allocation's boundaries, which helps
packing and read-ahead. But it changes Fxfs's allocator and on-disk format: allocations split at
extent boundaries, and partial frees within an extent. Because addresses already encode an
extent and an offset, moving to it later stays possible.

This replaces doc-2's `ALLOC` and `ROOT`:

- addresses need no allocation;
- Fxfs's superblocks are at addresses 0 and 512 KiB, where Fxfs puts them.

### Branches, forks and immutable extents are invisible to the guest

To the guest, its address space is private and mutable: what it reads is what it last wrote
there. The host keeps it so as follows:

- **Branches.** Each VM's filesystem is a branch. A fork copies the parent's branch in O(1), by
  recording the parent and the fork point, and the child sees the parent's committed contents.
- **Copy-on-write.** Addresses are per branch. The first write to a shared extent after a fork,
  through `WRITE` or a writable mapping, copies the extent, or the pages written, into the
  writing branch. Neither branch sees the other's writes.
- **Immutable versions.** Committed data is immutable on the host: each commit makes new
  versions of what it changed, packed into segments in commit order. Upload and garbage
  collection work on versions, never on the guest's view.
- **Shared pages.** VMs on a host that read the same version of an extent map the same host
  pages, whether forks of one VM or VMs of one base image.

So doc-2's provisional decisions 2 (immutable extents) and 3 (fork privacy) are the host's
business. The guest's Fxfs keeps its in-place writes: superblocks, and the overwrite mode of
preallocated files.

### Both write paths are specified

`WRITE` copies data into the store. Writable mappings (`MAP` with `MAP_WRITE`, behind the
`DAX_WRITE` feature) let the guest store straight into the host's pages. Milestone 1 implements
`WRITE` first, and measures both (TASK-26.8).

## Device

- **Device ID:** `0x4658` ("FX"), outside the virtio IDs OASIS assigns. The transport is virtio
  1.x, over MMIO in libkrun.
- **Byte order:** every field is little-endian.

### Features

| Bit | Name | Meaning |
|---:|---|---|
| 0 | `VIRTIO_EXTENT_F_DAX` | Shared memory region 0 is a DAX window, and `MAP`/`UNMAP` work. |
| 1 | `VIRTIO_EXTENT_F_DAX_WRITE` | `MAP` accepts `MAP_WRITE`. Needs `DAX`. |
| 32 | `VIRTIO_F_VERSION_1` | Required. |

### Configuration space

```c
struct virtio_extent_config {
	u8 tag[36];               /* the filesystem's name, NUL-padded: the mount's source */
	le32 num_request_queues;  /* at least 1 */
	le32 block_size;          /* 4096: the alignment of READ, WRITE and FREE */
	le32 map_alignment;       /* the host's page size: the alignment of MAP and UNMAP */
	le32 max_transfer;        /* the most bytes one READ or WRITE moves */
	le32 max_segments;        /* the most data descriptors one request carries */
	le64 address_space;       /* bytes of addresses, a power of two */
};
```

The configuration never changes, so the device sends no configuration-change notifications.

### Virtqueues

Queues `0` to `num_request_queues - 1` are request queues, interchangeable. A driver may use one
per CPU. Each request is one descriptor chain:

1. the header, which the device reads;
2. the request's arguments, which the device reads;
3. for `WRITE`, the data, which the device reads;
4. for `READ`, the buffer for the data, which the device writes;
5. the response, which the device writes.

The driver may have as many requests in flight as the queues hold.

### Shared memory

With `VIRTIO_EXTENT_F_DAX`, shared memory region 0 is the DAX window: guest-physical memory
whose contents `MAP` sets. The transport gives its size, as virtio-fs's does.

## Requests

```c
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
| 1 | `READ` | `le64 address; le32 len; le32 reserved;` | device writes `len` bytes |
| 2 | `WRITE` | `le64 address; le32 len; le32 reserved;` | device reads `len` bytes |
| 3 | `FREE` | `le64 address; le64 len;` | none |
| 4 | `MAP` | `le64 address; le64 len; le64 window_offset;` | none |
| 5 | `UNMAP` | `le64 window_offset; le64 len;` | none |
| 6 | `COMMIT` | none | none |
| 7 | `INFO` | none | device writes `struct virtio_extent_info` |

### `READ`

Reads `len` bytes at `address`, both multiples of `block_size`. Addresses never written, or
freed, read as zeros. A `READ` sees every `WRITE`, and every store through a writable mapping,
completed before it was submitted.

### `WRITE`

Writes `len` bytes at `address`, both multiples of `block_size`. The data is in the store, and
visible to reads and mappings, when the request completes. It is durable as part of its epoch
(see Ordering). With the `WRITE_BARRIER` flag (bit 0), the write follows a barrier, as if a
`COMMIT` were submitted just before it: Fxfs's `PRE_BARRIER` writes.

### `FREE`

Frees `len` bytes at `address`, both multiples of `block_size`. Afterwards the range reads as
zeros. The host releases its storage once the `FREE`'s epoch is durable.

The driver must not write a range it has freed until the `FREE` has completed. Fxfs never reuses
space before the journal commit that frees it, and that commit follows a barrier, so this holds.

### `MAP` and `UNMAP`

`MAP` maps `len` bytes at `address` into the window at `window_offset`. All three are multiples
of `map_alignment`, and the range lies within the window.

- Loads through the mapping see the store's contents, coherent with `WRITE`: both use the
  host's page cache.
- The mapping is read-only unless the flags hold `MAP_WRITE` (bit 0), which needs `DAX_WRITE`.
  A guest's stores to a read-only mapping fault in the guest.
- Stores through a writable mapping are part of the current epoch.
- A `MAP` replaces whatever the window held there.

`UNMAP` makes `len` bytes of the window at `window_offset` inaccessible. Neither touches the
store.

### `COMMIT`

A barrier. Every request completed before the `COMMIT` was submitted, and every store through a
writable mapping before then, is ordered before every request submitted after the `COMMIT`
completes.

- **Without flags,** the `COMMIT` completes once the order is fixed, which does not wait for
  durability: doc-2's requirement 2, under which fsync keeps its ordering but a crash may lose a
  suffix.
- **With `COMMIT_DURABLE`** (bit 0), it completes once everything before it is durable: on local
  disk in milestone 1, uploaded once object storage arrives (milestone 3). Unmount and `sync`
  use it.

### `INFO`

```c
struct virtio_extent_info {
	le64 capacity;     /* bytes the store can hold for this filesystem */
	le64 used;         /* bytes it holds */
	le64 available;    /* bytes it can still take */
};
```

For `statfs`. The address space is not the capacity: it only says how far addresses go.

### Errors

| Status | When |
|---|---|
| `0` | Success. |
| `EINVAL` (22) | An unknown type or flag, a misaligned or out-of-range address, length or window offset, or an unsupported feature's flag. |
| `EIO` (5) | The store failed to read or write. |
| `ENOSPC` (28) | The store cannot hold more. |
| `EROFS` (30) | The store stopped taking writes for this branch: another host took its lease (milestone 4), or the host is shutting it down. Reads still work. |

## Ordering and durability

Requests on all queues are applied in any order, except across barriers. `COMMIT`s and
`WRITE_BARRIER`s divide the requests into **epochs**:

- A request completed before a barrier was submitted belongs to an epoch before it.
- A request submitted after a barrier completed belongs to an epoch after it.
- A request in flight while a barrier is may fall on either side.
- Stores through writable mappings belong to the epoch in which they were made.

After a crash of the guest, the VMM or the host, the store holds:

- everything from the epochs before some barrier B;
- any subset of epoch B's writes and frees, each block written whole or not at all;
- nothing after it.

That is how disks behave with barriers, and how Fxfs's journal recovers: checksums find where a
torn journal ends. With `COMMIT_DURABLE`, B is at least that commit. How far B can lag the latest
commit is the host's lag, bounded by its configuration (doc-2, requirement 11).

## What Fxfs's device layer becomes

The guest's `storage_device::Device` over this device (TASK-26.3, 26.5):

| Fxfs | Device request |
|---|---|
| `block_size`, `block_count` | `block_size`, and `address_space / block_size` |
| `read` | `READ`; file data is mapped through the window instead (TASK-26.6) |
| `write` | `WRITE`, with `WRITE_BARRIER` when Fxfs asks for `PRE_BARRIER` |
| `trim` | `FREE` (`supports_trim`) |
| `flush` | `COMMIT` |
| `close` | `COMMIT_DURABLE` |

Fxfs runs with barriers enabled. Its allocator sees a device of `address_space` bytes, and
`statfs` reports `INFO`'s capacity, not the address space.
