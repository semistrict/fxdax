---
id: doc-7
title: 'Object storage: how the host keeps branches in S3 and GCS'
type: specification
created_date: '2026-10-17 01:31'
updated_date: '2026-11-05 11:03'
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

## Where it stands (2026-10-17)

- **The protocol** is `crates/extent_remote`: writers, readers, branch records and forks, the
  collector (`fxdax-gc`), checkpoints as block-indexed tables, and `object_store` behind it (S3,
  GCS; RustFS in `scripts/test-rustfs.sh`). Its simulation (Testing) checks it.
- **Hosts** keep stores' branches with `fxdax::remote` (TASK-28.5): `extentd` attaches each store
  it opens to the bucket `FXDAX_EXTENTD_REMOTE` names. The uploader has the store close its open
  segment within an interval of the commits it makes durable, and PUTs the store's closed segments,
  and the extents of their own they name, as they are, up to 32 at once, acknowledged in order;
  commits stall at a lag bound in commits and in bytes. Only the snapshot an uploader starts from,
  of what the store holds that the branch doesn't, is read back from the store.
- **Opening elsewhere and forks** (TASK-28.7): an empty store of a branch (`fxdaxctl take`)
  takes it on attaching, holding its extents without their data, which is fetched as it is read; a
  store whose branch another host moved on refuses. `ExtentStore::fork` (`fxdaxctl fork`)
  forks a store's branch in the bucket once its commits are uploaded, the child inheriting its
  extents. `extentd`'s `OPEN` names a path, not a branch: `fxdaxctl` makes the store first.
- **Reading** (TASK-28.9): by 1 MiB parts of objects, each checked against the CRC its object
  carries; one GET per part however many reads want it; a host's GETs bounded (64, 64 MiB), demand
  before read-ahead; parts kept in a sparse cache of their own (`FXDAX_EXTENTD_CACHE`), evicted by
  punching holes, with a floor of free space; read-ahead doubling along sequential reads.
- **Local forks** (TASK-28.2): a local store is a lineage of branches, laid out as the bucket lays
  one out (`segments/<branch>/`, `extents/<branch>/`, `index/<branch>/`, a record per branch). A
  fork writes the child's record and nothing else; the child's state is its parent's at the fork,
  over which its index holds what it changes (TASK-31): in memory, a layer the parent's forks of
  that point share with the parent, one copy, and opening a fork reads and writes what it changed,
  whatever its parent holds. Each branch claims the files its extents are in, those it inherited
  through the layer it is over; a file goes once no branch claims it. A parent keeps its index
  files as of its forks while they live. A clone's or fork's directory is a handle naming the
  store's, which must stay where it is while the fork lives. Forks are deleted through
  `extent_store::delete_host` (`Storage::delete`, `fxdaxctl delete`), which drops their claims,
  and so is a store once no fork of it is left; removing a store's directory (`rm -rf`) while forks
  of it live is not supported. (Settled with the user on 2026-10-17: a store per lineage, over
  independent directories and forks that copy every file.)
- **Local segments and checkpoints** (TASK-28.3, 28.6): sealed extents are one 4 KiB-aligned run
  each; a segment closes when full, old, asked (the uploader), or at a fork, with its footer after
  its data, framed as bucket objects are (`extent_frame`); a branch is its newest checkpoint and
  the footers after it, locally as in the bucket. Local checkpoints are tables in the bucket's
  format (`extent_frame::table`). A closed local segment is the object uploaded,
  byte for byte (TASK-29.2, decision 1): the footer is the bucket's. The uploader gives the store
  its writer's lease epoch, the segment number to go on from and the segment to follow
  (`extent_store::Footing`), and the store numbers and chains its segments so, tags their commits
  with the writer's epoch, names its extents of their own at the writer's keys, and keeps each
  closed segment until it is uploaded. *Added 2026-11-03 (TASK-153.8):* whatever closes a
  segment, a commit, a write finding no room, the uploader between commits or a fork, its framing
  runs on a thread of its own and ends itself, the segment's framed file synced and its raw file
  going once the commits its footer names are durable: a commit's flush, the uploader's take, a
  fork and the branch's closing only wait for it. A local checkpoint names the segment open then, whose
  footer and those after it follow it; extents held without their data (taken from the bucket)
  are named by a checkpoint, which adopting them makes the next commit write. A store's segment
  may close between commits, when the uploader asks, once no write is in progress in it.
- **Compression** (TASK-29, settled with the user on 2026-10-17): every object holding data, a
  local closed segment or sealed extent and every object in the bucket alike, is framed by
  `extent_frame` in parts of 1 MiB, each compressed with zstd (level 1) on its own, or raw where
  that is no smaller, at 4 KiB-aligned offsets, with a table of where each is stored. A local
  segment is written raw while open (`open-segments/`), and framed when it closes; an extent of
  its own, raw until sealed (`open-extents/`). Guests never map store files: DAX windows map
  extentd's memory, which reads fill, so all data on disk and in the bucket is compressed, and
  memory holds it uncompressed. Reads decompress the parts they touch, a host's fetches each
  part with one GET of its stored bytes, and its cache keeps them as stored.
- **Not yet:** GCS end to end on GCE with read throughput (TASK-28.11), which needs a bucket.

## Decisions

Settled with the user on 2026-10-16:

1. **One format, locally and remotely.** The local extent store (TASK-26.10) keeps segments in the
   format they are uploaded in. A closed local segment is byte for byte the object uploaded, so an
   upload is a PUT of the file, and the NVMe cache is the same files, sparse.
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
5. **A fork is local at once, and remote once the parent's fork point is uploaded.** The child's
   `branches/` record is created only after the parent's uploads reach the fork point.
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
10. **Branches come first.** The store learns O(1) forks (doc-2's milestone 2 part) before uploads,
   because the remote layout needs lineage from the start.

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
| `moves/<key>` | the live extents of the object at `key`, compacted, and where in it each is | create-only, by the collector |

doc-2's separate `index/` deltas and `frees/` objects are gone: a segment's footer is its delta,
and its frees, and a branch's history can be rebuilt from its segments alone. A batch with no data
(frees, a root, forks) is a segment with only a footer.

Numbers are fixed-width hex, so listings sort.

**Formats** *(added 2026-11-03, TASK-141.2; doc-11, rule 3).* A branch's record says first the
format of its objects (`layout::FORMAT`, now 1), so that a reader of any format reads it before the
rest: one of a newer format is refused (`Error::Format`, naming the branch and the format) before
anything else of the branch is read or written. Every reader of a branch reads its record first: a
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
- **Owners** *(added 2026-11-03, TASK-153.9).* Every record names its branch's owner: a number
  its maker chose at random, unique to that making. A fork's host chooses its child's as it claims
  the child, and the fork's footer names it, so that whoever writes the child's record, the fork's
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
  stops at its next acknowledgement), gives the children of the forks in its state their records,
  then marks its record deleted. Deleting a fork's child before it has its record creates the record
  deleted. A deletion whose recovery of the branch finds something lost, the record deleted since
  by another deletion (a placement cut off deletes once the latest one's deletion ended the lease),
  is done: that one gave the children their records, and the collector may have reclaimed what
  recovering the branch needs since (TASK-178).
- **Deleted forks' records stay:** a parent's writer slow to record a fork could otherwise make the
  record again. Collecting them needs a bound on how long a writer can stall; they are small, and
  their number is bounded by the forks made.
- This replaced `copy_store`, which forked by copying every file: locally, a fork is a record in
  its parent's store (Where it stands).

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

**A host dying** at any step leaves a claim, as any fork's does (Forks), naming the point: opening
what it left finishes the fork, or fails once the point is gone; deleting it gives it a deleted
record, which a dead host's create landing late finds, and its update, being conditional, fails.
With its disk lost too, C stays a record being made, holding what P holds, until the collector gives
it up, once P goes, or an operator deletes it.

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

- **The pager must not block on a fetch.** Page-ins call a synchronous `ExtentSource::read` on two
  threads; with object storage behind it, a 50 ms GET would hold one of them. The source becomes
  asynchronous.
- **One fetch scheduler per host:** parts of 1 MiB of objects' data, keyed by (object, part), each
  fetched by its stored bytes, which the object's table says where are; concurrent
  misses on a part share one GET; misses go before read-ahead; about 64 GETs in flight within a
  memory budget. Bodies are streamed into place (`get_opts`, not `get_ranges`, whose 10-way fan-out is
  fixed).
- **The NVMe cache** is the local store's own files: sparse copies of segments and extents at their
  real offsets, their parts as stored (compressed), filled by fetches and emptied by punching
  holes. Uploaded segments are already
  there. Pinned until uploaded; evicted by CLOCK over parts, keeping 10% of the disk free. Every read
  from NVMe or object storage is checked against the footer's CRCs; a mismatch refetches once, then
  fails with `EIO`.
- **Read-ahead** doubles from 1 MiB to 8 MiB along sequential misses in an object, and takes a
  segment's neighbouring parts, since extents committed together are read together. Profiles of
  what a branch reads at boot, replayed on resume, can come later.

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
     record of its own (`failed_forks_deleted_anywhere`, its `twins`).
- **Each rule** has a test that turns it off and expects a seed to find what it violates: fences,
  links between segments, acknowledging after a lease read, minting from the epoch's range,
  recording forks after their acknowledgement, listing before reading states, finding forks in
  states, deleting an earlier writer's extents only after a later one's segment, readers
  checking for a newer checkpoint, points refusing writers, forks of points reading the point after
  their records and saying they are being made, the collector giving those up, its listing the
  records after the states, and records naming their owners. The collector going by the segments
  extents' keys name is shown needed by a test of its own
  (`extents_ahead_of_their_segments_need_keys_naming_them`), as are ways starting at extents'
  seals (`writers_reading_past_checkpoints_landing_after_they_took_over_need_ways_from_seals`, and
  `ways_from_seals`, found by `recreated`'s seeds) and writers naming what their checkpoints move
  before writing them (`writers_naming_what_their_checkpoints_move_need_moves_taken_first`). These tests search seeds: a change to the simulation can move the
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
  `passes_fenced_stop` (`NoPassesFencedStop`), and doc-14's `wal_follows` (`NoWalFollows`) and
  `wal_after_fence` (`NoWalAfterFence`); `newest_checkpoint` has a second, of what a fork inherited
  (`NoNewestCheckpointInherited`, TASK-177), and branch::delete's taking a deletion done by another
  as done, which is no field of `Rules`, one of its own (`NoDeletionsDoneByOthers`, TASK-178). It
  found TASK-173 and TASK-174 in the collector, and TASK-183: two deletions of a branch at once,
  a never-acknowledged fork's segment landing between their recoveries, the second records the
  fork's child after a pass, reading the branch deleted, let go of its origin; the spec models a
  deletion fencing the end it recovered to (`deletionsFence`, `NoDeletionsFence`), which the code
  lacks. The spec's header says where it differs from the code.
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
