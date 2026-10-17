---
id: doc-7
title: 'Object storage: how the host keeps branches in S3 and GCS'
type: specification
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 01:32'
---
# Object storage: how the host keeps branches in S3 and GCS

Status: agreed design, 2026-10-16. It turns doc-2's milestone 3 (m-5) into a plan, from a survey of
open-source systems that keep data in object storage (below). Tasks: TASK-28 and its subtasks.

doc-2 settled what milestone 3 must give:

- a commit is safe once uploaded, with a bounded lag (requirement 11);
- extents packed into segments, with an index, compacted to reclaim freed extents (12);
- fork lineage and frees in object storage, one writer per branch fenced by a lease, and no other
  service (13);
- memory and NVMe caches, both disposable (14).

This document says how. Where it differs from doc-2's sketch, it says so.

## Decisions

Settled with the user on 2026-10-16:

1. **One format, locally and remotely.** The local extent store (TASK-26.10) keeps segments in the
   format they are uploaded in. A closed local segment is byte for byte the object uploaded, so an
   upload is a PUT of the file, and the NVMe cache is the same files, sparse.
2. **The client is the `object_store` crate** (Apache-2.0/MIT), for S3 and GCS.
3. **When the lag reaches its bound, commits stall.** `COMMIT` stops completing; `WRITE`s and reads
   go on. A host that cannot reach object storage stops its guests' commits rather than lose more
   than the bound, as a hard NFS mount hangs.
4. **Takeover is explicit.** No lease expires and no host heartbeats. Whatever moves a VM takes its
   lease; the host that had it learns at its next upload, stops taking commits (`EROFS`) and stops
   the VM. It can lose at most its unuploaded suffix, which its guest was never promised.
5. **A fork is local at once, and remote once the parent's fork point is uploaded.** The child's
   `branches/` record is created only after the parent's uploads reach the fork point.
6. **Forks are of a branch's head only.** No snapshots or forks of past points, for now.
7. **No compression and no encryption** in this milestone: both break ranged reads that land
   straight in DAX pages.
8. **Garbage collection is its own process,** `fxdax-gc`, one per bucket prefix, holding a lease of
   its own. Any host can run it.
9. **Tests use RustFS** (Apache-2.0) as a local S3 server, and GCS on GCE. Real S3 once credentials
   are available.
10. **Branches come first.** The store learns O(1) forks (doc-2's milestone 2 part) before uploads,
   because the remote layout needs lineage from the start.

## Layout

Per bucket prefix, for every branch of every lineage:

| Key | What | Written |
|---|---|---|
| `segments/<branch>/<seq>` | sealed extents and a footer; `seq` dense from 0 | create-only, by the branch's writer |
| `extents/<branch>/<epoch>.<sequence>` | an extent of 4 MiB or more, raw | before the segment naming it |
| `checkpoints/<branch>/<seq>` | the branch's index as of segment `seq` | create-only, by the branch's writer |
| `branches/<branch>` | parent, fork point, state (live, deleted) | create; updated with a condition |
| `leases/<branch>` | the writer's epoch and a nonce | updated with a condition |
| `gc/lease` | the garbage collector's epoch | updated with a condition |
| `gc/segments/<epoch>.<n>` | compaction's output | by the garbage collector |
| `moves/<branch>/<seq>` | where a compacted segment's extents went | create-only, by the garbage collector |

doc-2's separate `index/` deltas and `frees/` objects are gone: a segment's footer is its delta,
and its frees. One PUT both commits a batch and fences a stale writer, and a branch's history can
be rebuilt from its segments alone. A batch with no data (frees, a root) is a segment with only a
footer.

`seq` and epochs are fixed-width hex, so listings sort.

## Segments

A segment holds sealed extents, each in one run, 4 KiB-aligned, from offset 0, so an extent's offset
in the object is the offset of a ranged GET and of a page in DAX. After the data, a footer:

- the branch, `seq`, the writer's lease epoch, and the commit epochs it holds;
- for each extent sealed in it: ID, offset, length, CRC32C, and a CRC32C per 64 KiB, so ranged reads
  are verified;
- the extents of their own committed with it: ID, length, CRCs;
- the frees committed with it, the branch's own extents and inherited ones;
- the root, if a commit in it set one, and the branch's next sequence;
- a fixed trailer: the footer's offset, length and CRC32C, a version, and a magic number.

A reader finds the footer with one suffix GET.

**Changes to the local store.** Today a segment holds pieces of open extents as they were written,
and only the index knows what is where (TASK-26.10).

- An extent whose pieces are not one run at its seal (rewritten ranges, interleaved `WRITE`s) is
  copied into one run then. The pieces it leaves are dead bytes, which compaction drops.
- Whether an extent is stored on its own is decided by its sealed length, not by its first write.
- A segment closes when full or when it reaches an age, once no write is in progress in it and the
  commit sealing its last extents is made. Its footer is appended then, and it is uploaded.
- A local file is removed only once the commit that made it unreferenced is uploaded. Today `flush`
  removes it as soon as that commit is durable locally, which would lose an extent freed before its
  upload.
- The local index (checkpoints and per-commit deltas) stays, for local crash recovery between
  uploads. Checkpoints are written in the remote format (below).

## Checkpoints

A checkpoint is a table sorted by extent ID, in blocks of 64 KiB with a CRC32C each, a block index
and a trailer, so a host looks an extent up with a suffix GET, the index, and one block, without
loading the whole index. It also holds the root, the next sequence, the segment it covers through,
and each segment's live bytes, which compaction reads.

The writer writes one every so many segments or bytes of footers, on clean shutdown, and at a fork.
A branch's state is its newest checkpoint plus the footers of the segments after it. The two newest
checkpoints are kept.

## Fencing

The lease says who should write a branch; the dense, create-only segment sequence enforces it.

- **Taking over** a branch: update `leases/<branch>` from epoch E to E+1 with a condition (S3
  `If-Match`, GCS generation), find N, the end of the contiguous run of segments, and create a
  footer-only segment at N+1 with epoch E+1. If N+1 exists, read it and try the next.
- **A segment whose epoch is below that of a segment before it is void.** Readers skip it, and the
  garbage collector deletes it. That covers a stale writer's uploads that land after the fence, and
  one that recreates a sequence number the garbage collector deleted.
- **The stale writer** finds out when a create fails and the segment there has a higher epoch. It
  answers `EROFS` from then on (doc-4) and its VMM stops the guest.
- **An ambiguous create** (a 5xx after the object landed, retried into a failed precondition) is
  resolved by reading the object back: if it is ours, the create succeeded.
- **Extents of their own** carry the epoch in their key, so writers of different epochs never
  collide; multipart uploads cannot be conditional.
- **Startup** probes that the store honours conditional creates and updates, and refuses to run if
  not.

## Uploads

Each branch's commits go through:

1. **Ordered:** `COMMIT` completes (doc-4).
2. **Durable locally:** within 50 ms, as now.
3. **Closed:** the segment is full or old enough (the lag's time bound).
4. **Uploaded:** segments and extents of their own upload in parallel (single PUTs up to 64 MiB,
   multipart for larger extents), extents before the segments that name them.
5. **Committed:** a segment counts once every segment before it is uploaded too. Then
   `COMMIT_DURABLE` waiters up to it complete, and local files it made unreferenced go.

Invariants: what is uploaded never exceeds what is durable locally; a branch's committed segments are
a contiguous prefix; an object is uploaded before anything that names it.

**The lag** is bounded in bytes (by the NVMe budget) and in time. Segments close early and uploads
speed up from half of either bound; at the bound, commits stall (decision 3). Uploads retry forever
with backoff, and the host reports a branch that is falling behind.

**Recovery on the same host** opens the local store as now, reads the remote end of the branch, and
uploads what is missing; local files are still there, since nothing unuploaded was removed. If
another epoch took over, the local suffix past the fence is dropped and the local files are a cache.

**Opening on another host** takes the lease, fences, loads the newest checkpoint's footer and index
and the footers after it, and serves the guest from an empty cache.

## Forks

- A fork seals the parent's open extents: it is a barrier.
- The child gets a new branch ID and records its parent and the fork point (the parent's segment
  `seq`). Locally that is all; remotely `branches/<child>` is created once the parent has uploaded
  through the fork point (decision 5).
- An extent's ID names the branch that minted it, so a child looks an inherited extent up in that
  branch's index, as of now: it is immutable, so any version of the index that has it is right.
- A free of an inherited extent drops only the freeing branch's claim. The parent's frees after the
  fork point do not touch the child, and the child's do not touch the parent.
- This replaces `copy_store`, which forks by copying every file.

## Reading

- **The pager must not block on a fetch.** Page-ins call a synchronous `ExtentSource::read` on two
  threads; with object storage behind it, a 50 ms GET would hold one of them. The source becomes
  asynchronous.
- **One fetch scheduler per host:** parts of 1 MiB of objects, keyed by (object, part); concurrent
  misses on a part share one GET; misses go before read-ahead; about 64 GETs in flight within a
  memory budget. Bodies are streamed into place (`get_opts`, not `get_ranges`, whose 10-way fan-out is
  fixed).
- **The NVMe cache** is the local store's own files: sparse copies of segments and extents at their
  real offsets, filled by fetches and emptied by punching holes. Uploaded segments are already
  there. Pinned until uploaded; evicted by CLOCK over parts, keeping 10% of the disk free. Every read
  from NVMe or object storage is checked against the footer's CRCs; a mismatch refetches once, then
  fails with `EIO`.
- **Read-ahead** doubles from 1 MiB to 8 MiB along sequential misses in an object, and takes a
  segment's neighbouring parts, since extents committed together are read together. Profiles of
  what a branch reads at boot, replayed on resume, can come later.

## Garbage collection

`fxdax-gc` is the only thing that deletes. Writers only add.

- **Liveness.** An extent is live while a live branch holds it: the branch minted it or inherited
  it, and has not freed it. The collector marks from every live branch's head and fork points, and
  sweeps. That mark-and-sweep is also the oracle the tests check faster schemes against.
- **Compaction** of a segment mostly dead: write its live extents to `gc/segments/<epoch>.<n>`,
  create `moves/<branch>/<seq>` saying where each went, wait a grace period, check the collector's
  lease, then delete the segment. Writers fold moves into their next checkpoint. A reader that gets
  404 for a segment reads its moves. The collector never writes a branch's segments or checkpoints,
  so each branch keeps one writer.
- **Orphans:** void segments, objects of an epoch beyond a fence, and compaction output no move
  names. An object whose epoch is the branch's current one is never an orphan: it may be uploaded
  and not yet committed.
- **Time** (grace periods, minimum ages, by the store's clock) only saves round trips and backstops;
  correctness does not depend on it.
- **Deleting a branch:** mark `branches/<branch>` deleted, take its lease at a final epoch so no
  writer resumes, and let the next collection drop its claims. Its index lives on while descendants
  hold its extents.
- A collection that cannot read a branch's metadata stops; it never treats what it could not read
  as garbage.

## Testing

Before the code it checks (doc-2's risk: "the reclamation rules need a model and tests before
code"):

- **A deterministic simulation** of writers, forks, takeovers and the collector over `object_store`'s
  in-memory store wrapped to inject faults: errors after an object lands, delays, reordering,
  crashes at every step. It checks that no live extent is ever lost and that every dead object is
  eventually deleted, against the mark-and-sweep oracle, for the cases the survey found:
  1. frees before and after a fork, by parent and child;
  2. chains of three generations, with the middle branch deleted;
  3. a fork racing collection and compaction;
  4. a takeover while the old writer is still uploading;
  5. crashes between a segment's upload and the next, and between a fork and its record;
  6. frees in commits never made durable;
  7. the collector crashing, running twice, or fenced mid-deletion;
  8. a reader on another host with an old index across compaction;
  9. a stale writer recreating a deleted object;
  10. IDs minted after a takeover.
- **RustFS** for the S3 backend, in the workspace tests.
- **GCE** for GCS, and for throughput: a host should reach 1 GB/s of ranged GETs, and the part size is
  chosen by measuring 256 KiB to 4 MiB.

## Survey

Sources read, in `~/src`:

| Project | License | Taken |
|---|---|---|
| SlateDB | Apache-2.0 | create-only sequences as commit and fence; ambiguous creates resolved by reading back; a boundary against recreated deletions (its RFC 0026); orphans of uploads not yet indexed (RFC 0029); block-indexed tables |
| Neon | Apache-2.0 | the epoch in every object name; deletes only by a validated collector; fork points pinning parents; ordered upload queue, indexes after the data they name |
| Graft | Apache-2.0 | data first, then a create-only commit; lookups across a fork chain |
| ZeroFS | AGPL-3.0 | self-describing segments with a footer; compaction thresholds; prefetch windows |
| lsvd-rbd, lsvd | GPL-2.0, none | self-describing log objects; checkpoint before deleting cleaned objects; cleaning by utilisation |
| JuiceFS | Apache-2.0 | per-32 KiB CRCs on the cache; 2-random eviction; uploaded blocks adopted into the cache |
| mountpoint-s3 | Apache-2.0 | prefetch windows; demand ahead of prefetch; a memory budget for fetches |
| Litestream, AutoMQ | Apache-2.0 | ordered acknowledgement of parallel uploads; backpressure at a lag bound |
| delta-rs, rocksdb-cloud | Apache-2.0, GPL-2.0 | what not to do: no handling of ambiguous creates; clones with nothing pinning their source |
| s3backer | GPL-2.0 | what not to do: an object per small block |
| object_store | Apache-2.0/MIT | the client: `PutMode::Create` and `Update`; no conditional multipart or delete |
