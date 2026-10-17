---
id: doc-7
title: 'Object storage: how the host keeps branches in S3 and GCS'
type: specification
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 02:51'
---
# Object storage: how the host keeps branches in S3 and GCS

Status: agreed design, 2026-10-16; the protocol (layout, fencing, forks, collection) settled by its
simulation (TASK-28.1, `crates/extent_remote`), 2026-10-17. It turns doc-2's milestone 3 (m-5) into a
plan, from a survey of open-source systems that keep data in object storage (below). Tasks: TASK-28
and its subtasks.

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
| `segments/<branch>/<seq>` | sealed extents and a footer; `seq` dense from 0 | create-only, by the branch's writers |
| `extents/<branch>/<epoch>.<sequence>` | an extent of 4 MiB or more, raw, uploaded by the writer of `epoch` | create-only, before the segment naming it |
| `checkpoints/<branch>/<seq>` | the branch's state as of segment `seq` | create-only, by the branch's writers |
| `branches/<branch>` | where it was forked from, whether it is a fork's, whether deleted | create; updated with a condition |
| `leases/<branch>` | the writer's epoch and who took it | created, then updated with a condition |
| `gc/lease` | the collector's epoch | likewise |
| `moves/<key>` | the live extents of the object at `key`, compacted, and where in it each is | create-only, by the collector |

doc-2's separate `index/` deltas and `frees/` objects are gone: a segment's footer is its delta,
and its frees, and a branch's history can be rebuilt from its segments alone. A batch with no data
(frees, a root, forks) is a segment with only a footer.

Numbers are fixed-width hex, so listings sort.

## Segments

A segment holds sealed extents, each in one run, 4 KiB-aligned, from offset 0, so an extent's offset
in the object is the offset of a ranged GET and of a page in DAX. After the data, a footer:

- the branch, `seq`, the writer's lease epoch, and the commits it holds, by tags unique to each;
- the segment it follows, by its `seq` and epoch: the last one before it that is not void
  (Fencing);
- for each extent sealed in it: ID, offset, length, CRC32C, and a CRC32C per 64 KiB, so ranged reads
  are verified;
- the extents of their own committed with it: ID, length, CRCs;
- the frees committed with it, the branch's own extents and inherited ones;
- the root, if a commit in it set one, and the branch's next sequence;
- the forks made at its end;
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
loading the whole index. It also holds the root, the next sequence, the last segment applied, the
forks, and each segment's live bytes, which compaction reads. (The simulation keeps it as one
record; TASK-28.6 gives it this format.)

A writer writes one every so many segments or bytes of footers, on clean shutdown, and at a fork,
recording there where the collector moved extents, and dropping forks whose children were deleted.

**A branch's state** is its newest checkpoint, then the footers of its segments after it, in order,
to the first one missing, skipping void ones. A branch with no checkpoint starts from its parent's
state at its fork: the parent's newest checkpoint up to the fork point, then the parent's segments
to it, or, with none, from the parent's own origin. **A reader that finds a segment missing** lists
the checkpoints again: if one newer than its own has come, the collector may have removed the
segment, which it covers, and the reader starts again; otherwise the branch ends there. The
collector removes segments only once a checkpoint covers them, so this is exact.

## Fencing

The lease says who should write a branch; the dense, create-only segments and their links enforce it.

- **Taking a branch** (on opening it, the first time or after a crash, here or elsewhere): update
  `leases/<branch>` from epoch E to E+1 with a condition (S3 `If-Match`, GCS generation), recover the
  branch's state, and create the **fence**, an empty segment of epoch E+1, at its end. A segment an
  earlier writer landed there first is the branch's: apply it, and put the fence after it.
- **Each segment names the one it follows**, and one that does not follow the last before it that
  is not void is **void**: readers skip it. An earlier writer's segment that lands after the fence
  follows none of the branch's, and so does a writer's that lands where one of its own before it did
  not.
- **Gaps:** segments past the branch's end, an earlier writer's landed past a gap, are void; the new
  writer fills the gaps between them with empty segments, so that its own follow without one.
- **A writer acknowledges a segment** once it and every one before it are created and a read of the
  lease, after them, shows the lease still its. A later writer took the lease after that, and looked
  for the branch's end after, so it found the segment and fenced after it.
- **Minting:** after recovering, the writer checks that the lease is still its, then mints extent IDs
  from its epoch's range, the epoch shifted above bit 40 (`EPOCH_SHIFT`). No later writer's segment,
  minted from a later range, is in what it recovered, and no other writer mints from its range, so
  no ID names two extents, even one an earlier writer uploaded after a later took over. Host caches
  keyed by extent ID rely on this.
- **A writer finds out it was taken over** when a lease read after its creates shows another epoch,
  or a create fails on a segment of a later epoch. It answers `EROFS` from then on (doc-4) and its
  VMM stops the guest. A create that fails on an earlier writer's segment, landed late where its own
  was to go, means its later segments are void: it takes the branch again, which keeps its guest.
- **An ambiguous create** (an error after the object landed, retried into a failed condition) is
  resolved by reading the object back: if it holds what was written, the create succeeded. Lease
  updates name who took the lease, so they are recognised the same way.
- **Extents of their own** carry their uploader's epoch in their key, so writers of different epochs
  never write the same key; multipart uploads cannot be conditional.
- **Startup** probes that the store honours conditional creates and updates, and refuses to run if
  not.

## Uploads

Each branch's commits go through:

1. **Ordered:** `COMMIT` completes (doc-4).
2. **Durable locally:** within 50 ms, as now.
3. **Closed:** the segment is full or old enough (the lag's time bound).
4. **Uploaded:** segments and extents of their own upload in parallel (single PUTs up to 64 MiB,
   multipart for larger extents), extents before the segments that name them.
5. **Acknowledged:** once every segment before it is uploaded too, and a read of the lease after
   shows it still the writer's (Fencing). Then `COMMIT_DURABLE` waiters up to it complete, and local
   files it made unreferenced go.

Invariants: what is uploaded never exceeds what is durable locally; a branch's committed segments are
a contiguous prefix; an object is uploaded before anything that names it.

**The lag** is bounded in bytes (by the NVMe budget) and in time. Segments close early and uploads
speed up from half of either bound; at the bound, commits stall (decision 3). Uploads retry forever
with backoff, and the host reports a branch that is falling behind.

**Recovery on the same host** opens the local store as now, takes the branch, and commits again
the local commits past the last the branch holds, if it holds the local ones up to there; local
files are still there, since nothing unuploaded was removed. If another host took the branch on
meanwhile, the local suffix is dropped and the local files are a cache. Extents committed again are
uploaded under the new writer's epoch; the collector can tell they are garbage, if they are freed,
only once a later writer takes the branch.

**Opening on another host** takes the lease, fences, loads the newest checkpoint's footer and index
and the footers after it, and serves the guest from an empty cache.

## Forks

- A fork is a barrier: it seals the parent's open extents and closes the parent's segment, at
  whose end it is recorded, with the child's new branch ID.
- **The child's record**, `branches/<child>` with its parent and fork point, is created once that
  segment is acknowledged (decision 5): by the parent's writer, or the next one, which records the
  forks in the state it recovers, or the parent's deletion. A child gets no record for a fork whose
  segment never made it into the branch.
- **Inheriting is free:** the child's state starts as its parent's at the fork point, and the
  child's first checkpoint records it in full. Extents keep their IDs and locations in the parent's
  objects, which the collector keeps while the child holds them.
- A free of an inherited extent drops only the freeing branch's claim: the parent's frees after the
  fork point do not touch the child, and the child's do not touch the parent.
- **Deleting a branch** takes its lease at a final epoch (no writer takes it again, and the one it has
  stops at its next acknowledgement), gives the children of the forks in its state their records,
  then marks its record deleted. Deleting a fork's child before it has its record creates the record
  deleted.
- **Deleted forks' records stay:** a parent's writer slow to record a fork could otherwise make the
  record again. Collecting them needs a bound on how long a writer can stall; they are small, and
  their number is bounded by the forks made.
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

`fxdax-gc` is the only thing that deletes; writers only add. A pass:

1. **Takes `gc/lease`**, at a new epoch, and stops before deleting if another pass took it since.
2. **Lists every object first.** What is written after the listing is no candidate, which is what
   makes reading branches' states after it safe: what they name was either listed, and is seen as
   needed, or was not, and is left. Without this, a child's lease written after its record was read
   would be deleted, and the child's next writer would mint IDs another had.
3. **Finds the live branches:** the records not deleted, listed again after reading them until no
   new one appears (a deleted branch's children got their records before it was marked), and the
   forks in live branches' states, whose children may lack their records yet.
4. **Recovers each**, keeping what the recoveries read (checkpoints, segments to replay, records of
   parents a child's state starts from). A branch whose recovery fails because it was deleted
   meanwhile is not live; any other failure stops the pass, which never deletes what it could not
   read.
5. **Marks** the objects holding the extents the states name, and the moves the way to them goes
   through, whose maps are needed even once their data moved on.
6. **Compacts** objects whose live extents are under a fraction of their data: segments no recovery
   reads, and moves. Their live extents go to `moves/<key>`, data and map in one object, written
   whole, so there is never compaction output without its map, and no orphan of it. A reader that
   finds an object gone reads its moves.
7. **Deletes** what is listed and not needed:
   - segments a checkpoint covers that hold no live extent, or were compacted a grace period ago;
     never a live branch's segments at or past its end, which may be uploaded and not yet in it;
   - checkpoints no recovery read;
   - extents of their own no state names, once a later writer's segment is in the branch (an
     earlier writer's, whose segment can no longer land in it), or minted and uploaded by the
     current writer and below the sequence its segments in the branch reached;
   - moves no state's way goes through, once compacted in turn or once their object is gone;
   - deleted branches' records no live branch's recovery read, unless a fork's; leases of branches
     with no live record.

**Time** (the grace period, by the store's clock) only spares readers that just looked an extent up
a round trip; correctness does not depend on it.

**Not yet:** merging small objects. Compaction writes each object's live extents to its own moves;
merging several into one needs the moves of each to name it, and a moves object can be deleted only
while none of them could still be written.

## Testing

The protocol was settled by a simulation before the code it checks (doc-2's risk: "the reclamation
rules need a model and tests before code"): `crates/extent_remote`, its `sim` module (TASK-28.1).

- **The simulation** runs writers on hosts for their guests, forks, takeovers (a second writer taking
  a branch from one still running), crashes that may lose a host's disk, branch deletions, and
  collectors that stop midway or run two at once, over a bucket whose requests take effect when a
  seeded scheduler says: in any order, failing before or after taking effect, or after whoever made
  them has gone. Each seed is deterministic.
- **After every step** it recovers every live branch from the bucket and checks it against what its
  guests did: a path of the branch's own commits from where it started, past every commit a guest
  was told is durable, holding exactly the extents those commits made, each reading back its bytes,
  following moves. The writers no later one took over read what their guests hold. No extent ID is
  minted twice.
- **At the end** it lets everything finish, takes every branch over twice more, and runs the
  collector until nothing changes: the bucket must hold only what the live branches need.
- **Each case** the survey found is a profile that makes it likely, run over hundreds of seeds:
  1. frees before and after a fork, by parent and child (`forks_and_frees`);
  2. chains of generations, with branches in the middle deleted (`forks_and_frees`);
  3. a fork racing collection and compaction (`forks_racing_collection`);
  4. a takeover while the old writer is still uploading (`takeovers_while_uploading`,
     `takeovers_racing_collection`);
  5. crashes between uploads, and between a fork and its record (`crashes_anywhere`);
  6. frees in commits never made durable (`crashes_anywhere`);
  7. collectors crashing, running twice, or fenced (`collectors_stopping_and_racing`);
  8. writers' indexes older than compactions (`collectors_stopping_and_racing`, the writers' reads);
  9. a stale writer recreating a deleted object (`takeovers_while_uploading`);
  10. IDs minted after a takeover (every profile).
- **Each rule** has a test that turns it off and expects a seed to find what it violates: fences,
  links between segments, acknowledging after a lease read, minting from the epoch's range,
  recording forks after their acknowledgement, listing before reading states, finding forks in
  states, deleting an earlier writer's extents only after a later one's segment, and readers
  checking for a newer checkpoint. These tests search seeds: a change to the simulation can move the
  seed that finds a violation past a test's range, which then fails; widen it, or make its profile
  likelier to find the case.
- `SIM_SOAK=<n>` runs the profiles over n times the seeds; `SIM_SEED`, `SIM_SEEDS`, `SIM_PROFILE` and
  `SIM_OFF` run the ignored test `one` to debug a seed.
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
