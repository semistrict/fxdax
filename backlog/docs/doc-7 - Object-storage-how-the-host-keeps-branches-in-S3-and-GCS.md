---
id: doc-7
title: 'Object storage: how the host keeps branches in S3 and GCS'
type: specification
created_date: '2026-10-17 01:31'
updated_date: '2026-11-08 19:59'
---
# Object storage: how the host keeps branches in S3 and GCS

Status: agreed design, 2026-10-16; the protocol (layout, fencing, forks, collection) settled by its
simulation (TASK-28.1, `crates/extent_remote`), 2026-10-17. It turns doc-2's milestone 3 (m-5) into a
plan, from a survey of open-source systems that keep data in object storage (below). Tasks: TASK-28
and its subtasks. *Rewritten 2026-11-07 (TASK-165.10)* for the host as it is since TASK-165 (doc-14):
a branch lives in the bucket alone; a host keeps, on local disk, its stores' write-ahead logs, of
what the bucket doesn't have yet, and its part cache, of copies of what it does; opening a store is
one recovery, from the bucket and the log. No local lineage, local checkpoint, index or marker is
left, and what this document said of them is gone from it; doc-14 says what went and why.

doc-2 settled what milestone 3 must give:

- a commit is safe once uploaded, with a bounded lag (requirement 11);
- extents packed into segments, with an index, compacted to reclaim freed extents (12);
- fork lineage and frees in object storage, one writer per branch fenced by a lease, and no other
  service (13);
- memory and NVMe caches, both disposable (14).

This document says how. Where it differs from doc-2's sketch, it says so: the index is the bucket's
checkpoints and segments' footers, and lineage is in the bucket alone.

## Where it stands (2026-11-07)

- **The protocol** is `crates/extent_remote`: writers, readers, branch records and forks, points,
  the collector (`fxdax-gc`), checkpoints as block-indexed tables, the WAL's rebase (`Wal::rebase`),
  and `object_store` behind it (S3, GCS, Azure Blob Storage; RustFS in `scripts/test-rustfs.sh`),
  or a directory of the host's (`file:///DIR`, `extent_remote::local`). Its simulation and a TLA+
  spec check it (Testing).
- **Every store has a bucket**, which its description names (TASK-165.6): on a host of fxdax's
  system, the host's (`FXDAX_EXTENTD_REMOTE`); an image of the fxdax CLI's, or a test's, a file
  bucket of its own beside its directory (`<image>.bucket`), which its clones share. A store's
  directory holds one branch.
- **On the host** (doc-14, `crates/extent_store`): the guest's writes go to the store's open
  segment, raw, and each commit's record to the segment's log, synced by the flush that makes the
  commit durable; a segment closes when full, at the upload interval (1 s), when a commit waits for
  its upload, at a fork, or as the uploader stops, and is framed on a thread of its own, its footer
  stamped by the writer's chain. Closed segments are the store's **write-ahead log** (WAL): each
  the very object it will be in the bucket, synced before its PUT. The **uploader**
  (`extent_store::upload`) PUTs them, and the extents of their own they name, up to 16 of a store's
  at once (`IN_FLIGHT`, 512 MiB of the host's upload budget over 32 MiB segments) and 64 requests
  for the host, acknowledges them in order after a read of the lease, and moves each acknowledged
  file into the host's **part cache** (`Cache::adopt`), or removes it. Commits stall at a lag bound
  in commits (4,096) and in bytes of WAL (1 GiB).
- **Opening a store**, here or anywhere, the first time or after a crash, is one recovery (doc-14,
  Opening a store): its writer takes the branch's lease with the placement's token, recovers the
  branch from the bucket and fences its end; a segment left open is closed again from its log; the
  WAL is put after the branch's end by its commits (Recovery on the same host). An empty store of
  a branch (`fxdaxctl take`) takes it so, holding its extents without their data, which is
  fetched as it is read.
- **Forks, points and deletion** are the bucket's alone (TASK-165.7): a fork is a commit of the
  parent's naming the child, whose acknowledgement gives the child its record; the child's
  directory is made only then, and opening it recovers the child from the bucket (Forks).
- **Reading** (TASK-28.9): by 1 MiB parts of objects, each checked against the CRC its object
  carries; one GET per part however many reads want it; a host's GETs bounded (64 requests,
  64 MiB), demand before read-ahead; parts kept in the host's part cache
  (`FXDAX_EXTENTD_CACHE`), across restarts, read least recently evicted first by punching holes,
  with a floor of free space; read-ahead doubling along sequential reads, to 16 parts (Reading).
- **Compression** (TASK-29, settled with the user on 2026-10-17): every object holding data, a
  WAL segment or extent and every object in the bucket alike, is framed by `extent_frame` in parts
  of 1 MiB, each compressed with zstd (level 1) on its own, or raw where that is no smaller, at
  4 KiB-aligned offsets, with a table of where each is stored. A segment is written raw while open
  (`open-segments/`), and framed when it closes; an extent of its own, raw until sealed
  (`open-extents/`). Guests never map store files: DAX windows map extentd's memory, which reads
  fill, so all data on disk and in the bucket is compressed, and memory holds it uncompressed.
  Reads decompress the parts they touch, a host fetches each part with one GET of its stored
  bytes, and its cache keeps them as stored.
- **Measured** on GCE against GCS and against file buckets (docs/measurements/
  real-bucket-guest-suites-2026-11-06.md), and against main before the restructure
  (docs/measurements/restructure-before-after-2026-11-07.md): the perf gate the same or better,
  forked DAX 58% faster and a VM 557 MiB cheaper; forks 12 ms; a cold host's page-ins of a fork's
  data slower than reading it from local files was (TASK-225).

## Decisions

Settled with the user on 2026-10-16:

1. **One format, locally and remotely.** The local extent store (TASK-26.10) keeps segments in the
   format they are uploaded in. A closed local segment is byte for byte the object uploaded, so an
   upload is a PUT of the file. *Revised 2026-11-06 (TASK-165.8, 165.9):* the closed segments are
   the WAL, and once acknowledged a segment's file leaves the store for the host's part cache,
   whose files are sparse copies of objects (Reading); a segment restamped by a rebase is restamped
   in memory, and its upload restamps the bytes (`extent_frame::restamp`).
2. **The client is the `object_store` crate** (Apache-2.0/MIT), for S3 and GCS, and Azure Blob
   Storage (*added 2026-10-19, TASK-46:* `az://container/prefix`, its conditional writes If-None-Match
   and If-Match on ETags, credentials from `AZURE_*`).
3. **When the lag reaches its bound, commits stall.** `COMMIT` stops completing; `WRITE`s and reads
   go on. A host that cannot reach object storage stops its guests' commits rather than lose more
   than the bound, as a hard NFS mount hangs.
4. **Takeover is explicit.** No lease expires and no host heartbeats. Whatever moves a VM takes its
   lease; the host that had it learns at its next upload, stops taking commits (`EROFS`) and stops
   the VM. It can lose at most its unuploaded suffix, which its guest was never promised.
   *Added 2026-10-18 (TASK-39.7):* a lease carries the **fencing token** of the placement that took
   it, its boot ID, which a later placement's exceeds; a writer takes the lease only with a token
   as high, so a host cut off takes no branch back from the placement after it (`ESTALE`). A
   store's copy on a host carries its placement's token (`fxdaxctl place`, `take … TOKEN`). Before
   a clean stop, the system waits for a store's uploads (`fxdaxctl sync`, the extent daemon's control API's `SyncStore`).
5. **A fork is made once the parent's fork point is uploaded.** The child's `branches/` record is
   created only after the parent's uploads reach the fork point. *Revised 2026-11-05 (TASK-165.7):*
   it was local at once, a record in the parent's local lineage; there is no local fork now, and the
   child's store is made once its record exists (Forks).
6. **Forks are of a branch's head only.** No snapshots or forks of past points, for now.
   *Refined 2026-11-02 (TASK-158):* a fork at a point in time is a point: a record naming a place in
   its parent (Points).
7. **Compression, no encryption** in this milestone. *Revised 2026-10-17 (TASK-29):* the first
   ruling, no compression, held because compressed bytes cannot land straight in DAX pages. They
   need not: the DAX window maps extentd's memory, which the pager fills by reading the store, and
   never maps store files. So each 1 MiB part of an object is compressed with zstd on its own
   (Segments), and a read decompresses only the parts it touches. *Level 1, measured 2026-10-19
   (TASK-29.4,* docs/measurements/gce-fs-bench-zstd-{0,1,3} and gce-fs-bench-pgbench-zstd-{0,1},
   c3-standard-8, Fxfs in the guest over DAX, branches in GCS): against parts stored raw, level 1
   cuts what pgbench uploads from 1.9 GiB to 124 MiB, initializes its database in 4.1 s rather
   than 5.0 and runs 14% more transactions per second (1,442 against 1,262); the fs bench's other
   workloads uploaded 705 MiB rather than 1.2 GiB, alike within noise but sequential writes, 6%
   slower (65.8 MiB/s against 70.3). Level 3 compressed them no further (705.5 MiB) and wrote 6%
   slower again (61.8 MiB/s). `FXDAX_ZSTD_LEVEL` sets another, 0 storing parts raw, to measure.
8. **Garbage collection is its own process,** `fxdax-gc`, one per bucket prefix, holding a lease of
   its own. Any host can run it.
9. **Tests use RustFS** (Apache-2.0) as a local S3 server, Azurite (MIT) as a local Blob Storage
   (`scripts/test-azurite.sh`), and GCS on GCE. Real S3 once credentials
   are available. *Added 2026-11-04 (TASK-165.1):* and a directory of the host's, `file:///DIR`
   (`extent_remote::local`): `object_store`'s `LocalFileSystem`, which refuses conditional updates,
   with them added under a `flock` of the bucket's; every bucket runs one conformance suite
   (`bucket::conformance`).
10. **Branches come first.** The store learned O(1) forks (doc-2's milestone 2 part) before uploads,
   because the remote layout needs lineage from the start. *Since TASK-165.7* lineage is the
   bucket's alone: a fork is O(1) in the bucket, a record and the parent's commit, and the child's
   opening recovers its state, O(extents) of the parent at the fork (doc-14, Decisions).

## Layout

Per bucket prefix, for every branch of every lineage:

| Key | What | Written |
|---|---|---|
| `segments/<branch>/<seq>` | sealed extents and a footer; `seq` dense from 0 | create-only, by the branch's writers |
| `extents/<branch>/<epoch>.<seq>.<sequence>` | an extent of 4 MiB or more, raw, uploaded by the writer of `epoch` before its segment `seq`, which names it: the key is the writer's declaration of what may yet name it, which the collector goes by (Garbage collection) | create-only, before the segment naming it |
| `checkpoints/<branch>/<seq>` | the branch's state as of segment `seq` | create-only, by the branch's writers |
| `branches/<branch>` | the format of the branch's objects, first; its owner; where it was forked from, whether it is a fork's, whether deleted, whether a point, whether its fork of a point is being made, whether a failed fork's claim | create; updated with a condition |
| `leases/<branch>` | the writer's epoch, who took it, and its placement's fencing token | created, then updated with a condition |
| `gc/lease` | the collector's epoch | likewise |
| `gc/marks/<epoch>` | what the mark-only pass of that epoch would have deleted, each object's key, size, version and time as listed (Garbage collection) | create-only, by the collector; removed by an operator |
| `moves/<key>` | the live extents of the object at `key`, compacted, and where in it each is | create-only, by the collector |

doc-2's separate `index/` deltas and `frees/` objects are gone: a segment's footer is its delta,
and its frees, and a branch's history can be rebuilt from its segments alone. A batch with no data
(frees, a root, forks) is a segment with only a footer.

Numbers are fixed-width hex, so listings sort.

**Formats** *(added 2026-11-03, TASK-141.2; doc-11, rule 1).* A branch's record says first the
format of its objects (`layout::FORMAT`, now 2: footers and checkpoints hold the capacity,
TASK-165.3), so that a reader of any format reads it before the rest: one of another format, newer
or older, is refused (`Error::Format`, naming the branch and the format) before anything else of
the branch is read or written. Every reader of a branch reads its record first: a
writer taking it, a host attaching it, the collector, whose pass stops, deleting nothing in a
bucket it can't read. Every framed object (segments, extents on their own, checkpoints, moves)
ends with its frame's format, the last byte of its trailer's magic (`fxdaxob1`), which a reader of
an older frame names rather than calls corrupt. A lease goes with its branch's record.

**Kinds of record** *(added 2026-11-03, TASK-160.14).* A record's flags say together one of six
kinds of branch, deleted or not: a root, of no parent; a fork's child with a branch of its own, its
fork made nowhere in the bucket, or one a deletion found no record of; a fork's child, of an
origin; a point, of an origin; a fork of a point being made, of the point's origin; and a failed
fork's claim, of an origin. Only a root has no `forked`; only a root and a branch of its own have no
origin; `point`, `forking` and `claimed` each go with an origin, one at most. Any other combination
is refused as corrupt as the record is read (`record::Kind`), and what a record allows (whether a
writer takes it, whether it is a given fork's child, whether the collector may drop it, whether
its recovery failing lost nothing) is asked of its kind. Every create or update of a record or a
lease is one compare-and-swap loop (`branch::transition`), its rules in what it makes of what it
finds.

## Segments

A segment's data holds sealed extents, each in one run, 4 KiB-aligned, from offset 0: an extent's
place is its offset in the data. The data is stored in parts of 1 MiB, each compressed with zstd on
its own, or raw where that does not shrink it, at offsets in the object that are multiples of
4 KiB. After the parts, a footer:

- the branch, `seq`, the writer's lease epoch, and the commits it holds, by tags unique to each;
- the segment it follows, by its `seq` and epoch: the last one before it that is not void
  (Fencing);
- for each extent sealed in it: ID, offset, length;
- the extents of their own committed with it, and where they are;
- the frees committed with it, the branch's own extents and inherited ones;
- the root, if a commit in it set one, and the branch's next sequence;
- the forks made at its end: each child's branch, its owner (Forks), and whether it is a point;
- then, as in every object that holds data (segments, extents of their own, compaction's moves,
  checkpoints), a table of each part's stored offset and length, the CRC32C of its stored bytes,
  and its encoding, and a trailer: the lengths of the data, of the parts stored and of the
  footer, the CRCs of the footer and of the table, and a magic number.

A reader finds the footer and the table with one ranged GET of the object's end (two if they are
longer than 4 KiB). A host fetching extents fetches whole parts, each with one ranged GET of its
stored bytes, checks each against its CRC, then decompresses it (Reading). Incompressible data
costs its table, 17 bytes a part, and nothing more. *Implemented differently
from the first plan, of per-extent CRCs per 64 KiB in the footer: CRCs of the object's parts move
with nothing (compaction recomputes them for what it copies, from data it checked), need no entry
per extent in checkpoints, and match what a host fetches.*

**On the host** *(rewritten 2026-11-07; doc-14).* A store's open segment holds pieces of open
extents as they were written, raw, and its log the commits made in it, each the `Contents` its
footer will hold. A commit seals the extents written before it, each copied into one run in the
open segment if its pieces are not one already (the pieces left are dead bytes, gone with the
segment's raw file), or into a file of its own if large; whether an extent is stored on its own is
decided by its sealed length. A segment closes once no write is in progress in it and the commit
sealing its last extents is made; its framing writes the footer, stamped by the writer's chain,
and syncs the framed file into the WAL, and its raw file and log go once that is durable. Nothing
the bucket has not acknowledged leaves the host: a free's data stays (`extent_store::kept`) until
the commit freeing it is acknowledged.

## Checkpoints

A checkpoint is a table sorted by extent ID, in blocks of 64 KiB with a CRC32C each, a block index
and a trailer, so a host looks an extent up with a suffix GET, the index, and one block, without
loading the whole index. It also holds the root, the next sequence, the last segment applied, the
forks, and each segment's live bytes, which compaction reads. (The simulation keeps it as one
record; TASK-28.6 gives it this format.)

A writer writes one every 64 segments (`upload::CHECKPOINT_EVERY`), on clean shutdown, and as an
acknowledgement carries forks, which the checkpoint makes, recording there where the collector
moved extents, and dropping forks whose children were deleted (32 children's records read at most
a checkpoint, in turn: TASK-92.4).
It names the extents moved where they went itself before it writes the checkpoint, only forward
along their ways (another checkpoint of its own may have taken them further): so a checkpoint of
the writer holding the lease names no extent further along its moves than the writer does, which the
collector relies on (Garbage collection, TASK-175).

**A branch's state** is its newest checkpoint, then the footers of its segments after it, in order,
to the first one missing, skipping void ones. A branch with no checkpoint starts from its parent's
state at its fork: the parent's newest checkpoint up to the fork point, then the parent's segments
to it, or, with none, from the parent's own origin. **A reader that finds a segment missing** lists
the checkpoints again: if one newer than its own has come, the collector may have removed the
segment, which it covers, and the reader starts again; otherwise the branch ends there. The
collector removes segments only once a checkpoint covers them, so this is exact. So does a reader
that found no checkpoint of a branch and finds what it went to instead gone, its parent's record or
state: if one of the branch's has come since, the collector, recovering the branch from it, may have
removed that, a deleted parent's record, and the reader starts again (TASK-177).

## Fencing

The lease says who should write a branch; the dense, create-only segments and their links enforce it.

- **Taking a branch** (on opening it, the first time or after a crash, here or elsewhere): update
  `leases/<branch>` from epoch E to E+1 with a condition (S3 `If-Match`, GCS generation), unless
  the lease's fencing token is higher than the taker's placement's (TASK-39.7), recover the
  branch's state, and create the **fence**, an empty segment of epoch E+1, at its end. A segment an
  earlier writer landed there first is the branch's: apply it, and put the fence after it.
- **Each segment names the one it follows**, and one that does not follow the last before it that
  is not void is **void**: readers skip it. An earlier writer's segment that lands after the fence
  follows none of the branch's, and so does a writer's that lands where one of its own before it did
  not.
- **A deletion fences too** *(added 2026-11-05, TASK-183):* having ended the lease and recovered
  the branch, it creates an empty segment of the final epoch at the end it found, applying a segment
  landed there first and fencing after it, before it gives the forks' children their records. A
  writer's segment in flight, never acknowledged, then lands nowhere in the branch, so every
  deletion of it finds the same forks: two at once (a placement cut off and the latest one), the
  second recovering past a fork's segment landed after the first marked the branch deleted, would
  record the fork's child after a pass, reading the branch deleted and no record of the child, let
  go of what it needs. A point, which no writer has, needs none.
- **Gaps:** segments past the branch's end, an earlier writer's landed past a gap, are void; the new
  writer fills the gaps between them with empty segments, so that its own follow without one.
- **A writer acknowledges a segment** once it and every one before it are created and a read of the
  lease, after them, shows the lease still its. A later writer took the lease after that, and looked
  for the branch's end after, so it found the segment and fenced after it. *Its* is its taking, the
  lease's holder with its epoch, not its epoch alone: the collector removes a deleted branch's
  lease, and a taking that read the record before the deletion creates it again, at epoch 1, before
  it finds the branch deleted (TASK-179).
- **Minting:** after recovering, the writer checks that the lease is still its, then mints extent IDs
  from its epoch's range, the epoch shifted above bit 40 (`EPOCH_SHIFT`). No later writer's segment,
  minted from a later range, is in what it recovered, and no other writer mints from its range, so
  no ID names two extents, even one an earlier writer uploaded after a later took over. Host caches
  keyed by extent ID rely on this.
- **A writer finds out it was taken over** when a lease read after its creates shows another epoch,
  or a create fails on a segment of a later epoch. It answers `EROFS` from then on (doc-4) and its
  VMM stops the guest. A create that fails on an earlier writer's segment, landed late where its own
  was to go, means its later segments are void: it takes the branch again, which keeps its guest.
  *Implemented 2026-11-06 (TASK-204):* the store's uploader (`extent_store::upload`) takes a lease
  of its own, fences the branch past the late segment, and puts the store's write-ahead log after
  the branch's end (`Store::rebase`), as opening the store does; a log another writer's commits
  moved past still fails (`Diverged`, `ESTALE`), and a later writer's segment still fences it.
- **An ambiguous create** (an error after the object landed, retried into a failed condition) is
  resolved by reading the object back: if it holds what was written, the create succeeded. Lease
  updates name who took the lease, so they are recognised the same way.
- **Extents of their own** carry their uploader's epoch in their key, so writers of different epochs
  never write the same key: each is a create-only PUT, and none is a multipart upload, which could
  not be conditional (an extent is 64 MiB at most).
- **Startup** probes that the store honours conditional creates and updates, and refuses to run if
  not.

## Uploads

Each branch's commits go through:

1. **Ordered:** `COMMIT` completes (doc-4).
2. **Durable locally:** within 50 ms at the median, and doc-12's crash bound at most (TASK-221).
3. **Closed:** the segment is full or old enough (the lag's time bound).
4. **Uploaded:** segments and extents of their own upload in parallel, each a single create-only
   PUT (an extent is 64 MiB at most; no multipart upload), extents before the segments that name
   them, up to 16 segments of a store at once within the host's upload budget (64 requests,
   512 MiB).
5. **Acknowledged:** once every segment before it is uploaded too, and a read of the lease after
   shows it still the writer's (Fencing). Then `COMMIT_DURABLE` waiters up to it complete, the WAL's
   base moves past it, durably, and its files leave the store for the host's part cache.

Invariants: what is uploaded never exceeds what is durable locally; a branch's committed segments are
a contiguous prefix; an object is uploaded before anything that names it.

**The lag** is bounded in commits (`Options::max_lag`, 4,096) and in bytes of WAL
(`Options::max_lag_bytes`, 1 GiB, what local disk holds until uploaded), and a durable commit waits
at most the upload interval (1 s) before its segment closes for the uploader; at either bound,
commits stall (decision 3). A request object storage fails is retried, from 50 ms, doubling, to
5 s apart (`extent_remote::store`), an ambiguous one resolved by reading back (Fencing); the host
reports each store's upload progress (`UploadStatus`: what is not yet acknowledged, and since when).
A writer displaced by a late segment takes the branch again (Fencing); any other failure of its
writer's stops the uploader, and its commits waiting to be durable fail.

**Recovery on the same host** *(rewritten 2026-11-07; doc-14, Opening a store)* is opening the
store: its writer takes the branch and fences it; a segment left open is closed again from the
whole records of its log, writes after the last durable commit dropped; and the WAL is put after
the branch's end by its commits (`Wal::rebase`). If the branch's last commit is one of the WAL's,
the WAL's segments to it landed: acknowledged now, into the cache. If it is the WAL's base, or the
last the WAL's segments landed to, the rest are restamped on the new chain after the fence and
upload as any closed segment: the guest keeps every commit made durable here. Otherwise another
writer's commits moved the branch on, and the store refuses to open (`Diverged`, `ESTALE`) until
`fxdaxctl take` drops its WAL, losing what its guest was never promised. Extents committed again
are uploaded under the new writer's epoch, at its keys.

**Opening on another host** is the same with an empty WAL, which never diverges: it takes the
lease, fences, recovers the newest checkpoint and the footers after it, and serves the guest from
the host's part cache, fetching what it lacks.

## Forks

- A fork is a barrier: it seals the parent's open extents and closes the parent's segment, at
  whose end it is recorded, with the child's new branch ID.
- **The child's record**, `branches/<child>` with its parent and fork point, is created once that
  segment is acknowledged (decision 5): by the parent's writer, or the next one, which records the
  forks in the state it recovers, or the parent's deletion. A child gets no record for a fork whose
  segment never made it into the branch.
- **Owners** *(added 2026-11-03, TASK-153.9).* Every record names its branch's owner: a number
  its maker chose at random, unique to that making. A fork's host chooses its child's as it makes
  the fork's commit, and the fork's footer names it, so that whoever writes the child's record, the fork's
  host, the parent's writers or the parent's deletion, writes the same bytes; a plain branch's is
  its creator's. A record is a fork's if it names the fork's owner: a fork's deletion deletes no
  other, and two forks naming one child, neither's segment landing, both taking a record of their
  own, no longer both succeed, which a create reading back identical bytes took for its own
  (TASK-79.82). Every create-only write's bytes are so unique to their writer: segments by their
  epochs, leases by their holders, records by their owners. An owner is no tenant: tenants are
  authorized on each host by their directory and their VM manager (TASK-154).
- **Inheriting is free:** the child's state starts as its parent's at the fork point, and the
  child's first checkpoint records it in full. Extents keep their IDs and locations in the parent's
  objects, which the collector keeps while the child holds them.
- A free of an inherited extent drops only the freeing branch's claim: the parent's frees after the
  fork point do not touch the child, and the child's do not touch the parent.
- **Deleting a branch** takes its lease at a final epoch (no writer takes it again, and the one it has
  stops at its next acknowledgement), fences it at the end it recovered to (Fencing), gives the
  children of the forks in its state their records, then marks its record deleted. Deleting a
  fork's child before it has its record creates the record deleted. A deletion whose recovery of
  the branch finds something lost, the record deleted since by another deletion (a placement cut
  off deletes once the latest one's deletion ended the lease), is done: that one gave the children
  their records, and the collector may have reclaimed what recovering the branch needs since
  (TASK-178).
- **Deleted branches' records stay**, forks' children's and roots' alike: a parent's writer slow to
  record a fork could otherwise make the record again, of a fork's child, or of a root that took the
  ID the fork's guest named and was deleted, naming what the collector reclaimed (TASK-174).
  Collecting them needs a bound on how long a writer can stall; they are small, and their number
  is bounded by the branches made.
- **On the host** *(since TASK-165.7, 165.9)*, a fork is the bucket's alone: the parent's store
  commits the fork, which closes its segment (`Store::commit_fork`); the fork waits for the
  segment's acknowledgement and the child's record (`Remote::forked`); only then is the child's
  directory made, a store holding nothing of its own, whose opening recovers the child from the
  bucket, its extents read from the parent's objects there or the host's cache
  (`host_store::fork`). A child whose ID another branch has fails with `EEXIST`, making nothing. A
  fork failing once its commit was made is deleted in the bucket, as the fork's alone
  (`Remote::unfork`, `branch::delete_fork`), so that its segment, landing late, makes nothing, and
  its directory goes; its host crashing part way leaves a directory with no store, which deleting
  finishes, and a fork that stands or never was, as the bucket has it. This replaced `copy_store`,
  which forked by copying every file, and then local lineage, a record in the parent's store.

## Points

*Added 2026-11-02 (TASK-158).* A snapshot must restore on any number of hosts at once, which a fork
by a commit of its parent's rules out: every host's copy would be a writer of one branch. A point is
immutable instead.

- **A point is a name for a state.** The fork that makes one, what a snapshot sends, is an ordinary
  fork of the VM's branch S, a commit of S's, whose footer lists the child in `points` too. Its
  record says `point` from the first, and names S at the segment `at`. It has no lease, no segment
  and no checkpoint, then or ever: no writer takes it (`Error::Point`), before touching a lease, so
  that no path can. Its state is S's at `at`, for ever.
- **A fork of a point is its child's record alone.** It names the point's origin as its own, so that
  to recovery and the collector the child is a fork of S at `at`, and nothing of the point's or S's
  is written. Any number of hosts do this at once, none's being any other's business.
- **A host's copy** recovers the state from the bucket, takes it if it holds nothing, and has no
  writer. Nothing it commits goes to the bucket. It can't go stale. A VM opening it to write fails
  with `EROFS`.

The steps of a fork of point P to child C:

1. C's record, created saying the fork is being made (`forking: P`).
2. A read of P's record: deleted, the fork fails.
3. C's record, updated with a condition: no longer being made. Only then does the fork succeed.

**What keeps C's data.** C needs just what P does. A pass that finds P live keeps that; so does one
that finds C. The danger is a pass that finds neither, or takes P for deleted with C unseen:

- A pass that read P deleted did so after the deletion. If step 2 found P live, step 1 came before
  the deletion, and so before that read, and the pass lists the records again after it reads any
  new one, so it finds C.
- It then gives C up if C is still being made, with a condition (`branch::abandon`): if that wins,
  step 3 fails and no one was told; if step 3 wins, C is live, and kept. So nothing being made
  stops a pass, whatever it names, nor stays for ever.
- A pass that listed the records before P's existed may read S's state after P was forked, restored,
  deleted and dropped from S's state by a checkpoint (the *fork deleted* rule of Checkpoints),
  and S freed what P held. So a pass lists the records **again after it reads the states**, and
  recovers what it finds, again as long as what it recovers has a checkpoint of its own, as only
  such a state can have dropped a fork. A record made after that holds what the pass kept, or what
  it never listed: a fork after a state it read inherits what that state holds or a later object;
  a fork before it, that state names.

**A host dying** at any step leaves C's record being made, or made, and a directory with no store,
which deleting removes (Forks): a record made stands as a fork's orphan, for
`fxdax-gc --delete-branch` (TASK-165.7); one being made holds what P holds until the collector gives
it up, once P goes, or an operator deletes it. A dead host's create landing late finds a deleted
record, and its update, being conditional, fails.

**A failed fork's claim.** The deletion of a fork whose child has no record makes one, live, in case a
later placement holds the child. That its parent has a segment of that number proves nothing: it may
not be the fork's. So the record says `claimed`, and a pass that can't recover a claim no writer has
deletes it and goes on: nothing ever read it. Every other `Lost` stops the pass.

**Any store makes one.** ForkStore's `point` makes a point of a store as a snapshot does, so that a
base, from `ImportImage` or anything else, forks on any number of hosts at once: each takes a copy of
the point (CreateStore of its branch) and forks that. The base keeps its writer, and what it writes
after reaches no fork of the point. A point of a point is refused (`EINVAL`): it would name the same
state, and its forks fork the point itself (TASK-158).

**Deleting a point** is deleting a branch: its forks keep what they inherited, by their records, and
its moves stay for as long as it lives, as it has no writer to record them in a checkpoint.

## Reading

- **The pager does not block on a fetch.** A page-in reads through the store, on the pager's
  threads: an extent in the open segment or the WAL from its file, one the bucket holds
  (`extent_store::RemoteData`) through the host's fetcher, asynchronously (`crate::cache`), so that
  a GET holds no thread while it waits.
- **One fetch scheduler per host** (`extent_remote::fetch`): parts of 1 MiB of objects' data, keyed
  by (object, part), each fetched by its stored bytes, which the object's table says where are;
  concurrent misses on a part share one GET; misses go before read-ahead; 64 GETs in flight at
  most, within 64 MiB, for the host's fetchers together. Bodies are streamed into place
  (`get_opts`, not `get_ranges`, whose 10-way fan-out is fixed). An extent the collector moved is
  read where it went. Every part is checked against its CRC; one that fails is read again, once,
  by its object's tail fetched anew, then fails with `EIO`.
- **The part cache** (`extent_remote::cache`, `FXDAX_EXTENTD_CACHE`), one per host, shared by its
  stores: sparse copies of objects at their keys (`fxdax-parts/<key>`), their tails and the parts
  kept, as stored (compressed), at their real offsets, filled by fetches and by acknowledged WAL
  files moved in whole (`Cache::adopt`), and emptied by punching holes. It keeps no index file:
  opening it rebuilds what it keeps from its files' tails, each checked, so a host's restart
  refetches nothing it had (TASK-165.4). The parts read least recently go first (a clock of uses,
  `by_use`), an object's tail with its last part, to keep within its capacity (extentd: unbounded)
  and a floor of the filesystem's space free (extentd: 4 GiB, `CACHE_FLOOR`). It holds copies of
  what the bucket has, never data the bucket lacks. Without a cache (`FXDAX_EXTENTD_CACHE` empty),
  every page-in of an extent in the bucket is a GET.
- **Read-ahead** follows each object's reads (1,024 objects at most): along sequential reads its
  window doubles, from 1 part to 16 (`fetch::AHEAD`), and a read out of sequence starts it again at
  1; the parts read ahead go into the cache, and are read only where a cache is. Profiles of what
  a branch reads at boot are recorded and replayed into the extent cache as a fork boots
  (`fxdax::profile`).
- **Not yet** (TASK-225): a host whose cache holds none of a fork's data, one forking another
  host's base or a fresh host after a host's loss, pages it in as a host without a cache did
  (r812, docs/measurements/real-bucket-guest-suites-2026-11-06.md): a ranged GET a page-in, one
  after another as the guest makes them, again after each eviction from the extent cache.

## Garbage collection

`fxdax-gc` is the only thing that deletes; writers only add. A live branch is one whose record says
so, whether or not a host holds it, which only hosts know: `fxdax-gc URL --branches --held FILE`
lists those no host holds, FILE naming what they do, and `fxdax-gc URL --delete-branch ID` deletes
one for the next pass to reclaim. A pass:

1. **Takes `gc/lease`**, at a new epoch, and stops before deleting if another pass took it since;
   so it does too, without failing, where it finds lost what a live branch needs: what it read
   before may be older than what the pass that took the lease read, deleting since (a point it read
   live, and the point's fork being made, read after the point's deletion). A pass not fenced can't
   find that: what a pass deletes, it read after it took the lease (TASK-180).
2. **Lists every object first.** What is written after the listing is no candidate, which is what
   makes reading branches' states after it safe: what they name was either listed, and is seen as
   needed, or was not, and is left. Without this, a child's lease written after its record was read
   would be deleted, and the child's next writer would mint IDs another had.
3. **Finds the live branches:** the records not deleted, listed again after reading them until no
   new one appears (a deleted branch's children got their records before it was marked), and the
   forks in live branches' states, whose children may lack their records yet. It gives up on forks
   being made of points deleted (Points). Once it has read the states it lists the records again,
   and goes on with what it finds, while what it recovers has a checkpoint of its own (Points).
   **A branch is recovered once by each origin it is named by**, its record's and each parent's
   state naming a fork to it, not once: a child the pass recovered by its parent's state, that fork
   refused, may have a record of another origin, a restore's that took the ID first, which only
   listing the records again finds; what that origin needs is what the child does. Whether a
   segment or an extent may yet land goes by every recovery of its branch (TASK-173).
4. **Recovers each**, keeping what the recoveries read (checkpoints, segments to replay, records of
   parents a child's state starts from). A branch whose recovery fails because it was deleted
   meanwhile is not live; any other failure stops the pass, which never deletes what it could not
   read.
5. **Marks** the objects holding the extents the states name, and the moves the way to them goes
   through, whose maps are needed even once their data moved on. A branch's writer names its
   extents where the checkpoint it recovered from, or its own since, said, which the state the pass
   read may name further along their moves: a checkpoint of another writer's landing after the
   writer recovered the branch (the writer it took the branch from, which doesn't know), or the
   parent's state a branch inherited them from, which its writer read as it was then. So **the way
   starts where each extent was sealed**, every moves on it kept, unless the state's checkpoint is
   by the writer holding the branch's lease, read after the checkpoint was listed: a writer taking
   the branch after that recovers from that checkpoint or a later one, which names its extents no
   less far along, and the writer that made it named them so before it wrote it. *Decided
   2026-11-05 (TASK-175):* rather than checkpoints counting only once a later footer names them
   (each footer naming its writer's newest), which needs the format, a rebase when a fence finds
   one, and replays from older checkpoints while the newest waits; or a grace period after the last
   pass that saw a state name the moves' object (time, which correctness doesn't depend on).
6. **Compacts** objects whose live extents are under a fraction of their data: segments no recovery
   reads, and moves. Their live extents go to `moves/<key>`, data and map in one object, written
   whole, so there is never compaction output without its map, and no orphan of it. A reader that
   finds an object gone reads its moves.
7. **Deletes** what is listed and not needed:
   - segments a checkpoint covers that hold no live extent, or were compacted a grace period ago;
     never a live branch's segments at or past its end, which may be uploaded and not yet in it;
   - checkpoints no recovery read;
   - extents of their own no state names, once no segment will name them: by what their keys
     declare, the writer that uploaded each and the segment that names it, an earlier writer's once
     a later one's segment is in the branch, and the current writer's once a segment is at its
     segment's place. Not by the branch's next sequence number: extents are committed in no order
     of their sequence numbers (TASK-79.3). *Decided 2026-11-03 (TASK-153.7):* a writer declares
     what it has in flight by these keys, written with the objects, rather than by an object of
     its own, which would have to land before what it covers and be read after the listing, two
     orderings more (`dead-ends/collector-in-flight-declarations.md`);
   - moves no state's way goes through, once compacted in turn or once their object is gone;
   - no record, deleted or not (Forks, TASK-174); leases of branches with no live record.

**Time** (the grace period, by the store's clock) only spares readers that just looked an extent up
a round trip; correctness does not depend on it.

**Watching before deleting** *(added 2026-11-08, TASK-264).* Two passes delete nothing, for an
operator to see what the collector would do before trusting it with a bucket:

- `fxdax-gc URL --dry-run [--grace S] [--compact-below F]` writes nothing at all. It does not take
  `gc/lease`, which would bump the epoch and fence a pass running meanwhile out of its deletes; it
  reads the lease's epoch as it starts instead, and says it was fenced if a pass took the lease
  before it ended, what it read being possibly older than that pass's deletes. Nor does it write
  what giving up on forks writes (a fork of a deleted point being made, a failed fork's claim): it
  takes them as given up on. It compacts nothing, and prints each object a pass would delete, then
  each it would compact by its listed size, `KEY KIND SIZE delete|compact`, and their counts and
  bytes. It **undercounts**: what a pass compacts it deletes a grace period after, which a dry run
  never finds doomed. With `--compact-below 0`, a pass deletes exactly what it lists.
- `fxdax-gc URL --mark-only [--every S] [--grace S] [--compact-below F]` is a pass in every respect
  but deleting: it compacts as one does (the compacted originals are the keys most worth watching,
  TASK-175), and instead of deleting writes `gc/marks/<epoch>`, the doomed objects as listed.
  *Decided:* it **takes `gc/lease`** (step 1). Its marks are ordered by the epoch, each pass's its
  own and later than every pass's before, so readers and the next mark-only pass find the newest by
  key; a pass that deletes, fenced by it, is late, never wrong; and where it finds lost what a live
  branch needs, it can tell a pass that deleted since (it was fenced, and stops, TASK-180) from a
  bug, which a pass not holding the lease could not. Fenced, it writes no marks, as a fenced pass
  deletes nothing. Nothing marked goes, so **storage grows**: originals and their moves both, and
  every marks object, until an operator collects for real or removes them. The collector never
  takes `gc/marks/` for a candidate, nor for a branch's.
  - **Readers warn.** Every reader of a bucket reads it through `store::Store` (extentd's remote
    stores, writers recovering, fxdaxctl, the collector), which holds the newest marks' keys,
    read again every 60 s by a listing of `gc/marks/` (and a read once it changed) alongside one of
    its reads, uncounted in its stats, never a round trip a read. A GET, ranged GET or HEAD of a
    marked key logs a `warn` with the key, its kind (naming its branch), the request, the marks'
    epoch and the current span's trace and span IDs; in extentd the log names the guest request too
    (`request_id`). A read costs one load with no marks, one set lookup with some. Listings are
    logged at `debug` only: they name what callers pass over (older checkpoints, segments a
    checkpoint covers) as well as what they go by, which they read, and which is warned of. A
    warning is a bug in the collector, unless the reader is one the protocol lets find the object
    gone and start again (a recovery a newer checkpoint restarts, a reader within the grace period).
  - **Resurrection.** Each mark-only pass compares what it finds doomed with the newest marks
    before its own: an object marked then, listed now at the same version, size and key, and not
    doomed, is warned of loudly, as no correct fork or recovery reaches what no live branch
    reached. Marks of a shorter grace period than the pass's are not compared: a compacted object
    takes longer to be doomed under a longer one.

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
- **Every acknowledged segment's objects exist:** after each step, what recovering each live branch
  read, and what holds each extent it names, or the moves saying where that went. Nothing writes a
  point: no segment, checkpoint or writer's lease of one exists.
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
  9. a stale writer recreating a deleted object, its create held past the deleting pass (the
     `recreated` profile: `stale_creates_landing_past_deletions`);
  10. IDs minted after a takeover (every profile);
  11. one point restored on several hosts at once, racing each other, passes of the collector and
     the point's deletion, on copies gone stale, hosts dying at each step (`restores`);
  12. two forks naming one child within a round trip, neither's segment landing, each taking a
     record of its own (`failed_forks_deleted_anywhere`, its `twins`);
  13. a restore of a point naming the child of a fork whose segment is not yet acknowledged,
     whichever's record comes first taking the ID (`restores_taking_forks_ids`, TASK-173);
  14. a root taking the ID of a fork's child whose segment is not yet acknowledged, and deleted,
     as the fork's parent is deleted and collectors run (`forks_ids_taken_by_roots_deleted`,
     TASK-174);
  15. branches forking, deleted twice at once, by a placement cut off and the latest one, their
     fork segments in flight (`branches_deleted_twice_while_forking`, TASK-183).
- **Each rule** has a test that turns it off and expects a seed to find what it violates: fences,
  links between segments, acknowledging after a lease read, minting from the epoch's range,
  recording forks after their acknowledgement, listing before reading states, finding forks in
  states, deleting an earlier writer's extents only after a later one's segment, readers
  checking for a newer checkpoint, points refusing writers, forks of points reading the point after
  their records and saying they are being made, the collector giving those up, its listing the
  records after the states, records naming their owners, its keeping deleted roots' records
  (`collection_needs_records_kept`, of `forks_ids_taken_by_roots_deleted`'s profile), and
  deletions fencing the branch (`deletions_need_fences`, of
  `branches_deleted_twice_while_forking`'s). The collector going by the segments
  extents' keys name is shown needed by a test of its own
  (`extents_ahead_of_their_segments_need_keys_naming_them`), as are ways starting at extents'
  seals (`writers_reading_past_checkpoints_landing_after_they_took_over_need_ways_from_seals`, and
  `ways_from_seals`, found by `recreated`'s seeds) and writers naming what their checkpoints move
  before writing them (`writers_naming_what_their_checkpoints_move_need_moves_taken_first`), and
  the collector recovering a branch by each origin it is named by
  (`children_restored_from_points_keeping_their_origins_need_recoveries_by_origins`: no seed
  reaches its window). These tests search seeds: a change to the simulation can move the
  seed that finds a violation past a test's range, which then fails; widen it, or make its profile
  likelier to find the case.
- **TLA+** *(added 2026-11-05, TASK-165.2):* `specs/bucket/Bucket.tla` models the protocol, with
  doc-14's WAL, at the level of what each step reads and writes, and TLC exhausts small models of
  it (`specs/bucket/*.cfg`) where the simulation samples. Each rule above is a member of its
  `Rules`, and a mutation of `specs/bucket/mutations` removes it and must find the violation it
  prevents, run by `scripts/check-tla.sh` (`make check-tla`): `acks_after_lease`
  (`NoAcksAfterLease`), `fence` (`NoFence`), `continuity` (`NoContinuity`), `epoch_sequences`
  (`NoEpochSequences`), `records_after_ack` (`NoRecordsAfterAck`), `candidates_first`
  (`NoCandidatesFirst`), `newest_checkpoint` (`NoNewestCheckpoint`), `forks_from_states`
  (`NoForksFromStates`), `orphans_after_successor` (`NoOrphansAfterSuccessor`), `tokens`
  (`NoTokens`), `delete_tokens` (`NoDeleteTokens`), `footers` (`NoFooters`), `fork_claims_ids`
  (`NoForkClaimsIds`), `fork_claims_landed` (`NoForkClaimsLanded`), `points_take_no_writer`
  (`NoPointsTakeNoWriter`), `forks_check_points` (`NoForksCheckPoints`), `forks_pending`
  (`NoForksPending`), `collector_abandons` (`NoCollectorAbandons`), `records_relisted`
  (`NoRecordsRelisted`), `records_owned` (`NoRecordsOwned`), `extent_keys_name_segments`
  (`NoExtentKeysNameSegments`), `ways_from_seals` (`NoWaysFromSeals`, TASK-175's trace),
  `moves_taken_first` (`NoMovesTakenFirst`), `leases_by_takings` (`NoLeasesByTakings`),
  `passes_fenced_stop` (`NoPassesFencedStop`), `recoveries_by_origins` (`NoRecoveriesByOrigins`,
  TASK-173), `records_kept` (`NoRecordsKept`, TASK-174), `deletions_fence` (`NoDeletionsFence`,
  TASK-183), and doc-14's `wal_follows` (`NoWalFollows`) and `wal_after_fence`
  (`NoWalAfterFence`); `newest_checkpoint` has a second, of what a fork inherited
  (`NoNewestCheckpointInherited`, TASK-177), and branch::delete's taking a deletion done by another
  as done, which is no field of `Rules`, one of its own (`NoDeletionsDoneByOthers`, TASK-178). It
  found three holes, each fixed as a rule since: TASK-173, a child the collector recovered once, by
  its parent's state, though a record found on relisting names a restore's origin
  (`recoveries_by_origins`); TASK-174, a deleted root's record dropped and made again by a parent's
  slow writer as a fork's child (`records_kept`); and TASK-183, two deletions of a branch at once,
  a never-acknowledged fork's segment landing between their recoveries, the second recording the
  fork's child after a pass, reading the branch deleted, let go of its origin (`deletions_fence`).
  The spec's header says where it differs from the code.
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
