---
id: doc-2
title: 'Guest-owned metadata: Fxfs in the guest kernel over a virtio extent store'
type: specification
created_date: '2026-10-13 19:53'
updated_date: '2026-10-13 22:50'
---
# Guest-owned metadata: Fxfs in the guest kernel over a virtio extent store

Status: agreed design, 2026-10-13. Milestone 1 is planned; later milestones are outlined. Some decisions
are provisional, and milestone 1 settles them (see Open decisions).

## Why

fxdax serves an Fxfs filesystem from the host to a guest over virtio-fs. File data is fast:

- DAX reads and writes are within 2x of ext4 in the guest;
- mmap reads match ext4.

Metadata is not. On GCE under nested KVM, creates, cold stats, readdir and unlinks are 8-55x slower than
ext4 (docs/measurements/gce-fs-bench-2026-10-10T2202Z). libkrun's own virtio-fs passthrough is 6-36x
slower too, so the cost is the transport, not fxdax.

Every uncached metadata operation is a round trip of ~80-190 µs:

- the guest notifies the device;
- the host thread wakes;
- fxdax handles the request, in 17-22 µs;
- the device interrupts the guest, which then takes two more exits to the VMM and a worker handoff.

Caching (TASK-25.9) and guest halt polling (TASK-25.10) cut the number and cost of round trips. They
cannot make an uncached `stat` or a `create` local, and common work such as `git status`, builds and
package installs is dominated by such operations.

A filesystem whose metadata lives in the guest does those operations in guest memory, as ext4 does. The
other requirements keep file data on the host:

- one copy of memory across forks of a VM;
- remote, durable storage;
- unbounded size.

## Requirements

Settled in a design review on 2026-10-13.

1. **The guest owns all metadata.** It is authoritative for the namespace, inodes and attributes, and it
   streams changes out asynchronously, in batches. The host stores metadata as opaque bytes and does not
   interpret it, for now.
2. **Durability is POSIX with ordering preserved.** A crash may lose a suffix of recent history, but the
   filesystem always recovers to a prefix of it. fsync keeps its ordering guarantees, but need not wait
   for the change to be durable.
3. **Size is unbounded.** Metadata is cached in guest memory and can be evicted and read back. It is not
   held resident whole.
4. **The filesystem runs in the guest kernel,** for ext4-speed metadata and so that file pages can be
   mapped straight from the host's shared cache.
5. **Fxfs's code, ported to the guest kernel.** The goal is to reuse upstream's tested logic, not just
   its design or format.
6. **The interface to the host is an object store, not a block device** (doc-4). The guest writes
   immutable extents under IDs it mints itself, records the IDs in Fxfs's tree, reads and maps
   extents by ID, and frees them. Storage grows and shrinks with what is stored, with no device size.
7. **File data is read through DAX,** from pages the host shares between all VMs and forks on the host.
8. **Live fork of a running VM** on Linux/KVM:
   - The parent keeps running.
   - The children are durable VMs; ephemeral forks are deferred.
   - Memory is one copy across forks. That includes file data that forks read after the fork, for
     example files from the base image that each fork reads later.
   - A fork costs O(1), whatever the filesystem's size.
9. **Linux hosts only.** The VMM is libkrun: fxdax's device, DAX window, userfaultfd faults and KVM
   pre-faulting already work there. It gains KVM snapshot and fork, modelled on Firecracker's.
10. **The host persists to remote object storage,** with native support for both GCS and S3. This lets:
    - a VM resume or fork on any host;
    - durability go beyond one disk;
    - capacity be cheap;
    - a base image be shared across hosts.

    Guest RAM in object storage is out of scope for now.
11. **A commit is safe once uploaded,** with a bounded lag: a host failure loses at most the unuploaded
    suffix. Local disks are only caches.
12. **Extents are packed into large segment objects** in commit order, with an index, and compacted to
    reclaim freed extents.
13. **Fork lineage and free lists live in object storage,** with no extra service. Each branch (VM) has one
    writer, fenced by a lease taken with conditional writes.
14. **Each host caches in two tiers:** memory, as the shared pages mapped into VMs, and local NVMe. Both
    tiers are disposable.
15. **fxdax evolves into this design** rather than being kept alongside it.

## Architecture

```
 guest kernel                         host (libkrun + fxdax)                 object storage
┌───────────────────────────┐        ┌──────────────────────────────┐      ┌─────────────────────┐
│ VFS                       │        │ extent-store device          │      │ segments (packed    │
│  └ Fxfs (ported)          │ virtio │  ├ extent index per branch   │ GCS  │  extents, commit    │
│     ├ LSM tree, journal   │◄──────►│  ├ shared page cache (memory)│ ◄──► │  order)             │
│     ├ object/dir layers   │  DAX   │  ├ local NVMe cache          │  S3  │ index objects       │
│     └ extent-store client │◄──────►│  ├ uploader (ordered, lag)   │      │ lineage, free lists │
│ virtio extent-store driver│ window │  └ lease per branch          │      │ leases              │
└───────────────────────────┘        └──────────────────────────────┘      └─────────────────────┘
```

### Guest: Fxfs as a Linux filesystem

Fxfs keeps everything in one place:

- its objects, their attributes and extents;
- its directory entries, as records in a per-volume LSM tree;
- a journal of mutations, applied by transactions;
- an allocator.

In the guest these run as upstream wrote them, with three changes:

- **Storage layer.** Fxfs's `Device` and allocator assume a fixed-size block device whose ranges it
  allocates. They become an extent-store client (doc-4):
  - extent records hold `(extent ID, offset)`, not device offsets;
  - the allocator mints IDs, counts references per extent, and frees extents once unreferenced;
  - nothing is written in place: superblocks go to new extents that a root record names, each
    journal flush writes new extents, and overwrite mode becomes copy-on-write.
- **Data path.** File pages are not in the guest's page cache. A read maps the extent's range from the DAX
  window, which is one copy on the host shared by every VM. For writes, see Open decisions.
- **Runtime.** Fxfs's async code, its allocations and its Fuchsia-specific dependencies are adapted to
  the kernel (Rust for Linux). How is the first spike.

Metadata (the tree's layers, journal and superblock) is stored in extents like any data, so it is opaque
to the host. The guest caches the tree in its own memory and evicts it like any cache, reading layers
back from extents.

### The virtio extent-store device

Specified in doc-4 (TASK-26.2): efficient access to object storage from within the guest's kernel,
with Fxfs its first client. It is a paravirtual device rather than a block device because an object
store has no finite size, and because the host's memory and NVMe caches can then be shared between
every VM on the host for any extents they have in common.

- Extents are immutable once the next barrier seals them, named by 128-bit IDs (the branch and a
  sequence number the guest advances), so minting needs no request.
- `WRITE`, `READ`, `MAP`/`UNMAP` and `FREE` work on extents by ID.
- `COMMIT` is a barrier, which can also set the branch's root record. It does not wait for durability
  (requirement 2) unless asked to, for unmount and `sync`.
- `ROOT` reads the root at mount, and `INFO` reports capacity.

Many requests can be in flight at once. Metadata operations issue none once metadata is cached: only
data I/O, freeing, mapping, commits and mount do.

### Host: the extent store

**Extents and branches.** Each VM's filesystem is a branch, with its own ID and root. An extent is
visible to the branch that wrote it and to every branch forked from it afterwards. Extents never
change, so sharing them needs no copying.

**Fork.** Forking a VM forks its branch:

- The child branch records its parent and the parent's commit sequence at the fork.
- Nothing is copied, so the fork is O(1).
- Extents created before the fork are shared, and extents created after it are private to whichever
  branch created them.

A shared extent is reclaimed only when every branch that inherited it has freed it or been deleted, as
with back-references in btrfs snapshots. Each branch records its own frees, and garbage collection
decides.

**Page cache.** The shared pages are the in-process kernel's pager-backed memory, keyed by extent. The
same machinery serves fxdax's DAX windows today:

- userfaultfd faults;
- write-protect dirty tracking;
- KVM pre-faulting;
- the VMA-replacement race fix with EFAULT retry.

Every VM and fork on the host maps the same pages for the same extent. Behind memory sits a cache on
local NVMe, and behind that, ranged reads from object storage with read-ahead.

**Uploads and durability.**

- Sealed extents under 4 MiB are appended, in commit order, to an open segment, which is uploaded when
  it fills or ages out. Extents of 4 MiB and more are uploaded as objects of their own. The configured
  lag bounds how much a host failure can lose.
- An upload carries the branch's lease epoch.
- After the upload, the host writes the branch's index delta, which maps extent IDs to their segment,
  offset and length.

**Object storage layout,** per filesystem lineage:

- `segments/<branch>/<seq>`: small extents, packed in commit order.
- `extents/<branch>/<sequence>`: extents of 4 MiB and more.
- `index/<branch>/<seq>`: index deltas and periodic checkpoints.
- `branches/<branch>`: the parent, the fork point, and the state (live or deleted).
- `frees/<branch>/<seq>`: the extents the branch freed.
- `leases/<branch>`: the epoch, held with conditional writes (S3 `If-Match`; GCS generation
  preconditions).

**Garbage collection** reads branches, indexes and frees to find extents that no live branch can still
reach. It compacts segments that are mostly garbage and records its progress with conditional writes. It
coordinates through object storage alone, with no database.

**Resuming and forking elsewhere.** Another host takes the branch's lease by bumping the epoch, which
fences the old host. It reads the root and index, and serves the guest from its own caches, fetching from
object storage on a miss. Guest RAM moving with it is out of scope for now: a resumed VM boots cold,
with only its filesystem.

## Open decisions

Provisional choices, which milestone 1 or the fork milestone settles:

1. **The write path.**
   - DAX writes with barriers on commit: no copies and no request per write, with ordering explicit at
     commits. The provisional choice.
   - `WRITE` requests: one copy each, and ordered by the queue.

   Milestone 1 measures both.
2. **Immutable extents.** Settled by doc-4: extents are write-once, sealed by the next barrier, and
   Fxfs no longer writes in place.
3. **How a fork's writes stay private.** Settled by doc-4: each branch mints extents under its own ID,
   and shared extents never change. The reclamation rules are finalised before fork work starts.

## What fxdax keeps and what changes

**Kept:**

- the in-process Zircon kernel as the host's page cache (VMOs, pager, eviction, memory pressure);
- the DAX window machinery: the second stage, userfaultfd, pre-faulting, the EFAULT retry;
- libkrun integration and the device loop on fxdax's executor;
- the benchmark and its GCE infrastructure.

**Changed:**

- Fxfs moves from the host to the guest, behind a new device layer.
- The FUSE server and virtio-fs device give way to the extent-store device.
- Host persistence moves from an image file to object storage, through local caches.

## Milestones

1. **Guest Fxfs on a local extent store.**
   - Fxfs ported into the guest kernel, over a new virtio extent-store device in libkrun backed by host
     memory and local disk.
   - No object storage and no forks.
   - DAX reads.
   - Measures metadata operations and `git status` against ext4 in the guest, and settles the write
     path.
2. **KVM fork in libkrun.** Snapshot and live fork with copy-on-write guest RAM, with branches and O(1)
   extent forks in the store.
3. **Object storage.** Segments, index, uploads with bounded lag, leases, garbage collection, the NVMe
   cache tier, and GCS and S3 backends.
4. **Resume and fork on any host.**

## Risks

- **Porting Fxfs to the kernel** is the largest unknown. It means:
  - adapting the async executor;
  - adapting allocation;
  - stubbing or porting Fuchsia libraries;
  - binding the VFS from Rust, whose upstream support is limited.

  The first task is a spike to size it.
- **Extent IDs in Fxfs's tree** change its on-disk records, replace its allocator, and end its
  in-place writes: a large deviation from upstream that has to be kept maintainable. fxdax's image
  files become a local extent store, so one Fxfs serves the host and the guest.
- **Faults per 4 KiB page** under nested virtualization limit cold DAX reads (TASK-25.3). Shared extents
  favour 2 MiB host pages, which conflicts with fine-grained copy-on-write.
- **Mapping the same physical pages into several VMs** permits cross-VM cache side channels (as with
  KSM). This is acceptable for a single tenant; multiple tenants need review.
- **Garbage collection over object storage** with O(1) forks is subtle: the reclamation rules need a model
  and tests before code.
