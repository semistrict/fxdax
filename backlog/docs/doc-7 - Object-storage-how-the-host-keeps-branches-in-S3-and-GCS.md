---
id: doc-7
title: 'Object storage: how the host keeps branches in S3 and GCS'
type: specification
created_date: '2026-10-17 01:31'
updated_date: '2026-11-13 12:49'
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
  stamped by the writer's chain; in place, if compressing saves under an eighth, its data written
  once.
  One filled between commits waits for the next, which seals in it what is one run there, a commit
  spanning segments (Segments, TASK-92.41). Closed segments are the store's **write-ahead log**
  (WAL): each the very object it will be in the bucket, synced before its PUT. The **uploader**
  (`extent_store::upload`) PUTs them, and the extents of their own they name, up to 16 of a store's
  at once (`IN_FLIGHT`, 512 MiB of the host's upload budget over 32 MiB segments) and 64 requests
  for the host, acknowledges them in order, their creates the fencing, and moves each acknowledged
  file into the host's **part cache** (`Cache::adopt`), or removes it. Commits stall at a lag bound
  in commits (4,096) and in bytes of WAL (1 GiB). What waits is on the host's disk; of it, the
  host holds in memory only what it uploads at once, its **upload memory** (512 MiB,
  `FXDAX_EXTENTD_UPLOAD_MEMORY`), however far behind the bucket is (Uploads, Memory).
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

*Rewritten 2026-11-11 (TASK-364, format 5):* every branch is a volume of a tenant's **cell**, and
its objects are under the cell's prefix, `tenants/<tenant>/cells/<cell>/`, the tenant and the cell
each an ID in 16 hexadecimal digits, which callers keep the meaning of. The flat layout before it,
`segments/<branch>/...` at the bucket's root, is refused by format (`layout::FORMAT` 5): nothing
reads it.

Per bucket prefix, `<C>` a cell's prefix `tenants/<tenant>/cells/<cell>/`:

| Key | What | Written |
|---|---|---|
| `<C>cell` | the cell's object: who holds it, at what epoch, by which placement's fencing token, its volumes by name (each a branch of the cell's, and a kind: `fxfs` or `sqlite`), whether it was deleted (Cells) | created, then updated with a condition |
| `<C>segments/<branch>/<seq>` | sealed extents and a footer; `seq` dense from 0 | create-only, by the branch's writers |
| `<C>extents/<branch>/<epoch>.<seq>.<sequence>` | an extent of 4 MiB or more, raw, uploaded by the writer of `epoch` before its segment `seq`, which names it: the key is the writer's declaration of what may yet name it, which the collector goes by (Garbage collection) | create-only, before the segment naming it |
| `<C>checkpoints/<branch>/<seq>` | the branch's state as of segment `seq` | create-only, by the branch's writers |
| `<C>branches/<branch>` | the format of the branch's objects, first; its owner; where it was forked from, whether it is a fork's, whether deleted, whether a point, whether its fork of a point is being made, whether a failed fork's claim | create; updated with a condition |
| `<C>leases/<branch>` | the writer's epoch, who took it, and the token it was taken with: its cell's epoch (Cells) | created, then updated with a condition |
| `gc/lease` | the collector's epoch; per cell from format 8, `<C>gc/lease` (Garbage collection) | likewise |
| `gc/marks/<epoch>` | what the mark-only pass of that epoch would have deleted, each object's key, size, version and time as listed (Garbage collection) | create-only, by the collector; removed by an operator |
| `moves/<key>` | the live extents of the object at `key`, compacted, and where in it each is; under the cell from format 8, `<C>moves/...`, and references `<C>refs/...` with them (Garbage collection) | create-only, by the collector |
| `objectid/<block>` | a block of object IDs taken by one allocator, which gives its IDs out, each once (Object IDs) | create-only; kept for good |

A branch is named by its cell and its ID (`layout::Branch`, displayed `TENANT/CELL/ID`): footers,
fork children (`Child`) and origins (`Origin`) name branches so, so that recovery and the collector
reach a parent or a child in another cell, or another tenant's. An extent's ID names its branch by
the ID alone, as forks share their parents' extents by it and a host's extent cache keys them by
it, so a branch's ID is unique in its bucket, whatever its cell (Object IDs). The collector lists
`tenants/` whole for the records it reads again
(Garbage collection). A checkpoint's table stores its keys front-coded (`extent_frame::table`), as
a branch's keys share their cell's prefix and their kind's.

### Cells

A cell is a tenant's unit of placement: one host at a time holds it, and writes every volume of
it, each a branch of its own with a log, a lease and uploads of its own (one log per volume, the
user's choice of 2026-11-11, over one log per cell, which would have had the store multiplex
volumes). Taking a cell is one compare-and-swap of its object (`extent_remote::cell::take`): the
epoch after its last taking's, refused to a placement of a lower token than the one holding it
(`Stale`), refused once deleted. A volume's writer takes its branch's lease with the cell's epoch as
its token (`extent_store::upload`): a later taking of the cell, elsewhere, takes the volumes' leases
from it as any later writer does, and its writers learn of it at their next upload; it takes none
back, its epoch lower. So a cell moves whole, fenced by the branches' own leases, which cells change
nothing of (specs/bucket's model holds as it is). Every process of the host holding a cell takes it
as one holder, its placement's token, so that a second taking by that holder goes on at its epoch
rather than fencing the first: the extent daemon serving a cell's volumes to VMs and a FUSE mount
showing others share it.

A writer's opening of a store takes its cell (not a point's, which no writer takes and any
number of hosts open), making it if it has no object yet (a new branch, or a fork's child), and
has it hold its volume, named as its description says (`volume NAME`, `kind KIND`) unless the cell
names its branch already: a volume's name is its branch's from its first, whatever directory a
store of it is taken into. All in one compare-and-swap of the cell's object (`cell::hold`), a read
alone when nothing changes. Only the holder changes a cell's volumes, conditional on the epoch and
holder it took it at (`Fenced` otherwise). Deleting a branch takes its cell first, deletes the
branch with the cell's epoch, and takes its volume out of the cell. A cell's object stays, as
branches' records do: its epoch fences its volumes' leases, so made anew at epoch 1 it would leave
a branch whose lease a higher epoch took unopenable.

A store made alone, a VM's disk or an image, is the one volume of a new cell; a fork of one is the
one volume of a new cell of its tenant's. Forking a cell of several volumes whole, at one cut, is
the next step (TASK-364.1).

The extent daemon's tenants are the bucket's: a tenant's grant (the control API's `Grant`) names
its ID, and the stores in its directory are volumes of its own cells. Its VM processes make stores
of new cells of its own, and its VM manager the same (`CreateStore` and `ImportImage` of a new cell
there of another tenant are refused, `EPERM`; `ForkStore` into its directory makes a cell of its
own, whatever the parent's); they open no store of another tenant's cell (`EACCES`), refused by its
description before its opening would take its branch from the other tenant's writer. A point of
another tenant's the VM manager gives it (TASK-154), they restore from, each restore a store of a
new cell of their own. The daemon's own socket's VM processes, its user's, reach any store, and
make stores of tenant 0.

### Object IDs

Every new branch's ID is an object ID of its bucket's (`extent_remote::objectid`, TASK-364), which
no other branch of the bucket has, nor will: a store made, a fork, a point, a restore's child, a
volume a FUSE mount makes. Nothing names a new branch's ID itself: a VM process's `FORK` and the
control API's `ForkStore` name none, and are told the one given; `CreateStore` of a branch the
bucket has not is refused (`ENOENT`). Before cells, a branch's record, keyed by its ID alone, was
what refused a second branch of one ID; under cells, records are keyed by their cells too, and two
branches of one ID in two cells would share a host's cache entries, the one's VM reading the
other's pages.

An allocator, one per bucket in a process, takes a block of 2^24 IDs by creating its object,
`objectid/<block>`, which only one taking does; it gives the block's IDs in order, each once, an ID
costing no request but its block's first. A block is chosen at random, so that allocators taking
blocks at once seldom meet, and kept for good, as an ID is never given again: an allocator going,
its block's IDs not given go with it. A fork's child takes no ID of a branch its parent holds
extents of all the same (`extent_store::Inherited`), as a test's fork may name its child.

doc-2's separate `index/` deltas and `frees/` objects are gone: a segment's footer is its delta,
and its frees, and a branch's history can be rebuilt from its segments alone. A batch with no data
(frees, a root, forks) is a segment with only a footer.

Numbers are fixed-width hex, so listings sort.

**Formats** *(added 2026-11-03, TASK-141.2; doc-11, rule 1).* A branch's record says first the
format of its objects (`layout::FORMAT`, now 5: footers and checkpoints hold the capacity,
TASK-165.3, a footer the runs of a commit it goes on with, TASK-92.41, a checkpoint the branch's
fences, TASK-371, and a branch's objects are under its cell's prefix, TASK-364), so that a reader of
any format reads it before the rest: one of another format, newer or older, is refused (`Error::Format`, naming the branch and the format) before anything else of
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
origin; `point`, `forking` and `claimed` each go with an origin, one at most; `shared` goes with
`point` alone (fxdax, TASK-395.7: Shared points, under Points). Any other combination
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
- *added 2026-11-10 (TASK-92.41):* the runs in its data of the commit it goes on with past the
  commits it ends, if any, by that commit's tag (`Contents::continued`): a commit spanning
  segments (below);
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

*Added 2026-11-10 (TASK-310, TASK-92.41; the user's decision of 2026-10-23, commits spanning
segments, not footers naming runs in other objects):*

- **A segment framed in place.** One compressing its 1 MiB parts would save less than an eighth of,
  no write in progress in it, is framed where it is, every part stored raw: its raw file, its tail
  (footer, table, trailer) appended and synced, is
  linked into `segments/` as its framed file, the raw name going once its commits are durable, as a
  raw file does. Its data is written once, as the guest wrote it, not raw and then framed again: a
  sandbox host's daemon wrote some 2.3 times what its guests did, its framings waiting seconds
  behind its own writes on a disk its VMs and bucket shared (TASK-310). One that compresses more is
  framed in a copy, compressed, as before. A crash before the link is durable leaves the raw file and log,
  which an opening closes the segment again from.
- **A commit spanning segments.** A guest writing more than a segment between two commits (a large
  file before its fsync) fills segments: each, full at a segment and an extent (TASK-79.2), takes no
  more writes and waits for the next commit, which seals each extent one run in it where it is,
  copying nothing, and closes it. Its footer names, after what the commits before made in it (the
  first may hold some), the runs of that commit in its data (`continued`), with the commit's tag;
  the commit ends in the open segment, whose footer names it, its frees, its root, its extents of
  their own, and its runs there. Extents not one run where they are (written in pieces with others'
  between, or across two segments) are copied into the open segment, and past its room into the
  next, which the commit goes on with in turn. Before, a segment filled holding no commit was set
  aside and the commit copied every extent in it out, into a file of its own each past the open
  segment's room (store-bench's burst: 640 own files, each created, framed, synced and uploaded),
  and one holding commits was closed, the extents written since copied out of it alike. Footers
  name runs in their own segments alone. Each segment's log holds its part of the commit, a record
  of its runs, checked or held as any record's runs are (`wal::LogRecord`); recovery drops a
  continuation no segment after it completes, one after another, to the one ending it: a crash
  between keeps the commit only whole. The segments upload once the commit is durable, as any
  segment of its commits, and the uploader's closing between commits closes the first segment
  filled if it holds commits (its extents written since then copied out by the next commit); its
  interval leaves one the guest is filling (filled, or past 8 MiB written since the last commit)
  for the next commit, up to 30 s (`upload::Options::filling_wait`), unless a commit waits for
  its upload.
- **Framed segments made durable together, off their framings** *(TASK-310)*. A framing reads,
  frames and writes its segment under a name of its own (`segments/<b>/framing-<n>`), syncing
  nothing, then hands it to a thread of the store's that makes the segments framed meanwhile
  durable at once: the directories of raw files and logs first, if a segment opened since they were
  synced; their framed files synced together; each renamed its segment's; the directory of framed
  files synced once. The segment's framing ends only then, so the uploader, a closing between
  commits and a branch's closing wait for it durable as before, and its raw file and log go no
  sooner. Each framing had synced its file, then the directories of framed and raw files, one
  after another, the next framing waiting: on a disk VMs share, each sync a journal commit and a
  flush of the disk's cache behind the others' writes, 0.3-1.4 s each with eight VMs writing. No
  flush syncs the directory of framed files. Recovery doesn't rely on framings being durable one
  after another: it removes `framing-` files; a whole framed file is its segment's record only
  while every segment before it is whole; after the first that isn't, a segment whose raw file and
  log are both still there is closed again from them; and what is kept ends at a log refusing a
  record's runs, at a segment holding no durable commit, or at a log not following the last commit
  kept (TASK-334): the commits a flush made durable are in the logs. A spanning commit's segments
  are let go of together once the last is acknowledged (TASK-338); one ending a commit and going
  on with the next, landed alone, a rebase stamps again with its continuation alone (TASK-345).
- **Readers** (`recovery::State::apply_contents`, the one replay) hold the runs a segment goes on
  with (`State::begun`) and seal them, where they are, as the segment ending the commit is applied,
  before its own; a segment after them that neither goes on with the commit nor ends it, a later
  writer's fence, leaves them unsealed: the commit, never acknowledged, never was. **Writers
  acknowledge only segments that end their commits** (`Writer::acknowledge`), so that what a store
  lets go of, once acknowledged, the bucket has whole, and no acknowledged state, nor any
  checkpoint, holds a commit begun. A WAL is put after a branch's end only between commits
  (`Wal::rebase`): its segments going on with a commit the fence left unended are no end of the
  branch, and are stamped again after it, with the rest of the commit.

## Checkpoints

A checkpoint is a table sorted by extent ID, in blocks of 64 KiB with a CRC32C each, a block index
and a trailer, so a host looks an extent up with a suffix GET, the index, and one block, without
loading the whole index. It also holds the root, the next sequence, the last segment applied, the
forks, and each segment's live bytes, which compaction reads. (The simulation keeps it as one
record; TASK-28.6 gives it this format.)

A writer writes one every 64 segments (`upload::CHECKPOINT_EVERY`), on clean shutdown, and as an
acknowledgement carries forks (each as of the last segment it acknowledged, which ends its commits,
TASK-92.41), which the checkpoint makes, recording there where the collector
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
- **A deletion reads the record again** *(added 2026-11-12, TASK-380):* once its recovery is
  fenced, before it gives the forks' children their records; deleted since, it is done. The fence
  holds only while it is there: a pass, reading the branch deleted, deletes it with what no live
  branch needs, the extents a writer taken over stored on their own ahead of its segment among it.
  That segment, of a fork, landing where the fence was, and a second deletion, which read the
  record live before the first marked it, recovering past it (the branch's segments kept to an
  earlier fork, whose child lives), would record the fork's child, which names the extents the
  pass deleted. Found live by that read, the record shows the recovery and its fence came before
  any deletion marked it, so before any pass let go of a fence.
- **Gaps:** segments past the branch's end, an earlier writer's landed past a gap, are void; the new
  writer fills the gaps between them with empty segments, so that its own follow without one.
- **A writer acknowledges a segment** once it and every one before it are created: its creates are
  its fencing. *Revised 2026-11-12 (TASK-371):* it acknowledged them only once a read of the lease,
  after them, showed the lease still its, a second round trip for every durable commit (a guest's
  sync 3 a second at 25 ms of latency, of which these reads were a third). **The rule that makes
  that safe, once** *(TASK-388, replacing three rules found one lost commit at a time: TASK-371's
  two and TASK-383's)*: **the collector never frees a position a writer of a live branch, taken
  over or taking it over, may still create at, for as long as such a writer may live**
  (specs/bucket's `CreatesNeverFreed`). It is enough: a writer creates its segments in order and
  acknowledges them in order, so it acknowledges nothing past the first create that fails. A
  writer taken over fails at the first position past its own segments, as its successor's
  recovery found every position before the successor's fence occupied, by its own segments or by
  another's; so all it acknowledged is what its successor applied, as long as those positions stay
  occupied. A writer taking over fences at the end it read, so all the last writer acknowledged
  before is in what it applies, as long as what landed at and past that end stays until it fences.
  The collector computes those positions in one function (`collector::pinned`), from a live
  branch's state and its lease read after the listing: while a writer takes the branch over, its
  lease of an epoch past the state's, **every segment**; otherwise **every fence**, the first
  segment of each epoch its state applied, of every epoch, and **every stray**, a segment its state
  read that is neither a fence nor of the epoch of the fence before it: void (an earlier writer's
  landed late, following none applied) or late (an earlier epoch's following the last applied, as
  an earlier writer's fence made after its successor's does). A writer's own segments but its fence
  go once a checkpoint covers them: it creates each once. Checkpoints carry the fences and strays
  (`State::fences`, `State::strays`); nothing says when no writer of an epoch can be left, so the
  pins stay, a few objects per takeover. Each case is a rule a test turns off (`pins_fences`,
  `pins_old_fences`, `pins_void_strays`, `pins_late_strays`, `pins_takings`), and specs/bucket's
  mutation of each finds an acknowledged commit lost. A deleted branch keeps no pins, and its writer may then
  acknowledge a commit no branch holds, which no one reads; but **a segment holding a fork is
  acknowledged only once a read of the lease, after its creates, shows the lease still the
  writer's** (`forks_after_lease`), as recording the fork's child would name segments the
  collector let go of. *Its* is its taking, the lease's holder with its epoch, not its epoch alone:
  the collector removes a deleted branch's lease, and a taking that read the record before the
  deletion creates it again, at epoch 1, before it finds the branch deleted (TASK-179).
- **Minting:** after recovering, the writer checks that the lease is still its, then mints extent IDs
  from its epoch's range, the epoch shifted above bit 40 (`EPOCH_SHIFT`). No later writer's segment,
  minted from a later range, is in what it recovered, and no other writer mints from its range, so
  no ID names two extents, even one an earlier writer uploaded after a later took over. Host caches
  keyed by extent ID rely on this.
- **A writer finds out it was taken over** when a create fails on a segment of a later epoch, its
  successor's fence or after it; or, acknowledging a fork, when a lease read after its creates
  shows another taking. It answers `EROFS` from then on (doc-4) and its
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
   them, up to 16 segments of a store at once within the host's upload budget (64 requests, its
   upload memory: 512 MiB unless `FXDAX_EXTENTD_UPLOAD_MEMORY` says otherwise; Memory, below).
5. **Acknowledged:** once every segment before it is uploaded too, its create the fencing; one
   holding a fork, once a read of the lease after shows it still the writer's (Fencing, TASK-371); a segment going on with a commit, with the segment
   ending it (Segments, TASK-92.41). Then `COMMIT_DURABLE` waiters up to it complete, the WAL's
   base moves past it, durably, and its files leave the store for the host's part cache.

Invariants: what is uploaded never exceeds what is durable locally; a branch's committed segments are
a contiguous prefix; an object is uploaded before anything that names it.

**The lag** is bounded in commits (`Options::max_lag`, 4,096) and in bytes of WAL
(`Options::max_lag_bytes`, 1 GiB, what local disk holds until uploaded), and a durable commit waits
at most the upload interval (1 s) before its segment closes for the uploader; at either bound,
commits stall (decision 3). A request object storage fails is retried, from 50 ms, doubling, to
5 s apart (`extent_remote::store`), for as long as it fails, an ambiguous one resolved by reading
back (Fencing); the host reports each store's upload progress (`UploadStatus`: what is not yet
acknowledged, and since when). *Added 2026-11-10 (TASK-314):* the store counts the requests that
fail by kind and logs each kind's run of failures with the bucket's own error, as it starts, every
30 s, and as it ends, so that a bucket failing for good (full, out of reach, refusing its
credentials) says why its guests' commits wait; doc-12 says what the guest sees.
A writer displaced by a late segment takes the branch again (Fencing); any other failure of its
writer's stops the uploader, and its commits waiting to be durable fail.

**Memory** *(added 2026-11-10, TASK-317).* A bucket that takes less than the guests write costs a
host disk, not memory: what waits is the stores' write-ahead logs, durable on its disk (decision 3),
within the lag bound, and the uploader takes at most `IN_FLIGHT` segments of a store's at a time,
reading none until its upload goes. What a host's extent daemon holds in memory for it is bounded
by its **upload memory**, `FXDAX_EXTENTD_UPLOAD_MEMORY` (512 MiB by default, 64 MiB at least, an
extent's most, which goes alone), every store's uploads together: each object read only once that
budget has room for it (`extent_remote::fetch::Budget`), held once while its PUT goes, however
often it is retried (`Bucket::put` takes `bytes::Bytes`, which each try shares), and let go as it
lands. Besides, the daemon holds, whatever the backlog, each store's framing of one segment, its
data and its framed bytes (~2-4 segments' size, TASK-325), the host's fetches (64 MiB in flight,
64 MiB of tails, 64 MiB of parts recent), its connections' rooms (doc-4) and its extent cache
(`MEMORY_BYTES`, shared memory). The stats line's `uploads:` says what the uploads hold, the most
they held and the budget, and so do the metrics `fxdax_extentd_upload_held_bytes`,
`fxdax_extentd_upload_held_max_bytes` and `fxdax_extentd_upload_memory_bytes`.

What the daemon frees, it gives back: glibc's allocator maps each allocation of 1 MiB or more on
its own and unmaps it as it is freed (`extentd::memory::return_large_allocations`, mallopt's
`M_MMAP_THRESHOLD`). By default glibc raises that threshold to the largest buffer freed, up to
32 MiB, and keeps buffers below it in its arenas, one a thread, up to eight times the CPUs.
*Measured on ubu1* (fxfs-guest-docker-build-dax, 8 vCPUs, three builds of 512 MiB; `MEMORY_BYTES`
2 GiB; a file bucket on disk behind 300 ms a request; commits stalled at the lag bound 8 times,
extentd in a systemd scope of its own, scripts/test-e2e.sh's `FXDAX_EXTENTD_MEMORY_MAX`): before,
its anonymous memory reached 5.9 GB (4.4 GB under heaptrack, and 4.8 GB idle after that run),
its heap 1.3 GB at most: its 512 MiB of uploads held twice, a copy for each PUT's try, and 2.6 GB
in 134 of glibc's per-thread arenas of 64 MiB, buffers freed; the host's OOM killer took it at
5-6 GB on a loaded host (TASK-311). After, 666-718 MB at most in two runs, its uploads holding
510 MiB at most of their 512 MiB, and the run passed in a scope limited to 3 GiB, its 2 GiB extent
cache included, with no kill.

*Added 2026-11-10 (TASK-330.2):* the daemon holds itself to that sum, its **anonymous memory's
budget** (`fxdax::extentd::process`): its upload memory, a store's framing and sealing for each
store written at once, the most so far (twice a segment and an extent, TASK-325's whole-segment
framing, and a commit's copy of an extent, 288 MiB), its fetches (192 MiB), its
tenants' rooms, as much as they held at most, and its own (64 MiB). It samples its anonymous
memory (`RssAnon`) as it keeps its stats, every 200 ms, against that budget as of then, and counts
the samples past it: the stats line's `process:` (`max-anon`, `anon-budget`, `over-budget`), and
the metrics `fxdax_extentd_process_anon_bytes`, `_anon_max_bytes`, `_anon_budget_bytes` and
`_anon_over_budget_total`, what an operator alerts on. A sample past it is memory no part holds, a
copy no budget counts or what the allocator kept: both of TASK-317's bugs. The guest tests fail a
run whose daemon passed it (`scripts/budgets.py`), and the daemon's soak against a degraded bucket
holds it to it (`extentd::tests::soak`, TASK-330.3).

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
2. A read of P's record: deleted, the fork fails. fxdax: so does it if C is of another tenant's cell
   than P, and the record doesn't say P is shared (`Error::NotShared`, `EACCES`; TASK-395.7). The
   read is made then whatever the rule that checks for deletion says.
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

**Shared points** *(fxdax, TASK-395.7, format 8).* A point forks only into cells of its own tenant's,
unless its record says `shared`: a base image or a snapshot many tenants clone. Only a point's record
says it (another's is corrupt), and only the trusted control API sets it, at the point's making
(ForkStore's `shared`, after the point is made) or later (SharePoint, `fxdaxctl share`), and clears
it (`unshare`). The check is step 2's read, which every fork of a point makes, on any host, so that
unsharing stops the forks whose reads come after, and those made stand. A fork refused leaves C's
record being made, which its host deletes, as for a point deleted. No tenant is special: tenant 0's
points are shared or not as any other's. Forks of branches with writers into another tenant's cell
(a public base forked into a tenant's directory) are not checked yet (TASK-395.9).

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

*Rewritten 2026-11-13 (TASK-395.1): per cell, conservative.* The protocol below is model-checked
in `specs/bucket` (Bucket.tla, the `BucketCells*` models and every other model extended to cells)
and is the design of record for TASK-395.2-.8; the code still runs the whole-bucket pass it
replaces (one `gc/lease`, every object listed, every live branch recovered), which goes when the
per-cell pass lands, never beside it.

`fxdax-gc` is the only thing that deletes; writers only add. A live branch is one whose record says
so, whether or not a host holds it, which only hosts know: `fxdax-gc URL --branches --held FILE`
lists those no host holds, FILE naming what they do, and `fxdax-gc URL --delete-branch ID` deletes
one for the next pass to reclaim.

**Decided with the user (2026-11-13, TASK-395):** the collector may be **conservative**, reclaiming
late and wasting space for a time; **the unit of collection is the cell**, not the tenant, so that a
tenant of a million branches is collected as independently as a million tenants. Everything a
cell's collection decides by is under the cell's prefix: its branches' objects, their moves, its
collector lease, and **references** from forks in other cells. A pass over a cell lists nothing
else, and deletes nothing else but the references it retires; it reads other cells' objects
(GETs) only where its branches inherit from them, or to read a point's record.

### The layout (format 8, TASK-395.2)

`<C>` is a cell's prefix, `tenants/<tenant>/cells/<cell>/`; numbers fixed-width hex as elsewhere.

| Key | What | Written |
|---|---|---|
| `<C>gc/lease` | the cell's collector lease: its epoch and holder; replaces `gc/lease` | created, then updated with a condition, by passes over the cell |
| `<C>gc/marks/<epoch>` | what the cell's mark-only pass of that epoch would have deleted | create-only, by the collector |
| `<C>gc/...` | anything else a pass records to skip by (TASK-395.6) | by the collector; never a candidate |
| `<C>moves/<rest>` | the moves of the object at `<C><rest>`, and `<C>moves/moves/<rest>` those moves' moves: a reader of `<C><rest>` that finds it gone, or not naming the extent, reads `<C>moves/<rest>`, then `<C>moves/moves/<rest>` | create-only, by the cell's passes |
| `<C>refs/<parent>/<at>/<tenant>.<cell>.<child>` | a reference: the fork of the cell's branch `parent` at its segment `at` to `child` of cell `<cell>` of `<tenant>`, each ID 16 hex digits; its bytes the format and the fork's owner (a writable fork's, from its footer; a clone's, its restore's), unique to the fork, so that every writer of it writes the same | create-only (an object of identical bytes there is success), before the child's record; deleted by the passes of the child's cell |

A listing of `<C>refs/` with delimiter `/` gives the cell's parents with forks in other cells; of
`<C>refs/<parent>/` with delimiter `/`, that parent's distinct fork points
(`<C>refs/<parent>/<at>/`), which is what a pass keeps by; a fork point's children are listed only
when its state is found lost (below) or to count references. The child's key needs no listing to be
found: its record's origin names the parent, with its cell, and the fork point. `<C>cell`, `<C>gc/`
and `<C>refs/` are never a pass's candidates; branches' records are kept as before (Forks).

### References

A fork whose child is in another cell, as every fork, point and restore of the hosts' is (each makes
a new cell), writes a reference under the **parent's** cell, the cell holding what the child
inherits. For a clone of a point, that is the cell of the branch the point names (the point's
origin), as the clone's record names that origin as its own.

- **A writable fork** (or a point, which a fork of its branch makes): the reference before the
  child's record, by whoever writes the record: the fork's host, the parent's writers recording the
  forks they recover, the parent's deletion. A deletion writes its forks' references before it
  marks the parent deleted (rule `referencesFirst`).
- **A clone of a point**: the reference first, then the child's record being made (`forking`), then
  the read of the point's record (rule `clonesReferFirst`). A clone that finds the point deleted
  fails, its record deleted, and leaves its reference to be retired. A clone writes its reference
  **whatever cell it is in**, the origin's too (as no host makes: every restore is a new cell):
  the origin's cell's pass reads its records before the states and its references after, so a
  clone in that cell, made after the records were read, whose point (in another cell) is deleted
  and its own reference retired before the references are read, would be seen by neither
  (`BucketOverlap` found it, 2026-11-13).
- **A failed fork's claim** writes none: nothing must recover a claim (one that can't be is given
  up on). Writing one would be harmless.
- **Retired** by a pass over the child's cell, once that cell has **no live branch and no reference
  of its own** (rule `referencesHeld`): then nothing anywhere inherits from its children, as a fork
  of one of them in the cell has a record there, and one in another cell a reference there. The pass
  deletes, for each of its cell's deleted records with an origin in another cell, the reference that
  origin gives. A reference whose child's record names another origin (a child ID taken twice,
  which object IDs rule out) is never retired: space wasted, nothing lost.
- *Decided (TASK-395.1):* retiring is the **one delete a pass makes outside its cell**, a key it
  computes from its own records, never lists. Whether a reference may go is a fact of the child's
  cell (its records, its references), which only that cell's pass lists; the reference must be
  under the parent's cell, which the parent's pass lists. The two other ways: the child's pass
  writing a tombstone in its own cell that the parent's pass reads, or the parent's pass reading
  the child's record, each cost the parent's pass a GET per reference per pass (a million for a base
  cloned a million times) and leave the parent's listing unchanged when a child goes, so that
  TASK-395.6's idle pass would never see it. A retirement changes the parent's listing, which is
  what makes its next pass look again. The ordering is checked by `NoReferencesHeld`: retired once
  the child is deleted, its cell still holding the reference of the child's own fork, the
  parent's pass frees what the grandchild, live in a third cell, inherited through it.

### A pass over a cell

1. **Takes `<C>gc/lease`**, at a new epoch, and stops before deleting or retiring if another pass
   over the cell took it since; so it does too, without failing, where it finds lost what a live
   branch needs (TASK-180). Passes over other cells run at once, unfenced by it.
2. **Lists `<C>` once.** What is written under it after the listing is no candidate (rule
   `candidatesFirst`), and what is outside it never is: objects of other cells its branches name are
   kept by those cells, by reference.
3. **Reads the cell's records**, listed again until no new one appears, and gives up on forks of
   points deleted being made (Points), reading the point's record wherever it is.
4. **Recovers each live branch** of the cell, by each origin it is named by (TASK-173), reading other
   cells' objects where it inherits. Of the forks in the states it recovers, a child in the cell
   with no record yet is recovered by its parent's state, as before; for a child in another cell,
   the parent's state at the fork point is kept (below), as the child may lack its reference yet
   (rule `forksFromStates`).
5. **Lists the cell's records again** (while a state it recovered is from a checkpoint, as before)
   **and its references** (rule `referencesRelisted`), going back to 3 for new records. For each
   fork point of a reference it recovers the parent's **state at the fork point** once, however
   many references name it (a base cloned a million times is recovered once), and keeps it whole:
   the extents it names, the objects and moves holding them along their ways from where they were
   sealed (as a child with no checkpoint of its own names them where it read them), and what
   recovering it reads: the parent's checkpoint at or before the fork point, or its segments from
   its origin's state, and its record (rule `forkPointsKept`). What the child frees of it since is
   ignored: the conservative step. A fork point's state found lost fails the pass if a child named
   under it is live (its record, read then); one whose children are not (deleted, being made of a
   deleted point, no record yet) loses nothing.
6. **Marks, compacts and deletes** within the cell as the whole-bucket pass did (steps below), its
   live branches' states and the fork points' together. A cell with no live branch and no reference
   is reclaimed whole but its records and its cell object.
7. **Retires** its deleted children's references, if the cell has no live branch and no reference.

Why each ordering, as the model checks it (each rule's mutation in `specs/bucket/mutations` loses
an acknowledged commit's data: `NothingNeededGone`):

- A writable fork's child inherits the parent's state at the fork point from the parent's cell. A
  pass there reading the parent live finds the fork in its state (a checkpoint drops a fork only once
  its child is deleted) and keeps the fork point; one reading the parent deleted read it after the
  deletion marked it, which came after the fork's reference, and lists the references after
  (`NoReferences`, `NoReferencesFirst`).
- A clone that read the point live wrote its reference before the point's deletion. A pass over the
  origin's cell that keeps nothing for the point read the point deleted (a point of its cell), after
  the deletion, or found the point's own reference retired, after the deletion too; in both it lists
  the references after that, finding the clone's (`NoClonesReferFirst`). Listed with the
  candidates instead, the clone's comes too late where a checkpoint of the origin, dropping the
  deleted point, makes the pass recover the origin past the fork point (`NoReferencesRelisted`).
- A child's own forks inherit through it what it inherited: their references in its cell hold its
  reference in its parent's (`NoReferencesHeld`).
- A child with no checkpoint recovers through the parent's checkpoint or segments, which hold no
  extent of its own, at the fork point (`NoForkPointsKept`).

**What a pass over `<C>` requests outside `<C>`**, so that *per cell* is exact (everything else, its
lease, its LISTs, its other GETs and every PUT and DELETE, is under `<C>`):

| Request | When | How many per pass |
|---|---|---|
| LIST | never | 0 |
| GET of a parent's record, checkpoint or segments (another cell's) | recovering a live branch of `<C>` with no checkpoint of its own, or a fork point whose parent has none at or before it, through its origins until one has | per such branch, its ancestors' reads to their nearest checkpoint; 0 for a branch with a checkpoint |
| GET of a point's record | a clone in `<C>` still being made | one per such clone (they last one restore) |
| GET of a child's record | a fork point's state found lost | one per child of that fork point (never, when nothing is lost) |
| DELETE of a reference under the parent's cell | `<C>` has no live branch and no reference | one per deleted record of `<C>` with an origin elsewhere, until `<C>`'s listing stops changing (TASK-395.6), and none after |

None of them scales with another cell's size: what a pass costs grows with its own cell's objects,
branches and fork points, plus its branches' ancestry where it has no checkpoint. A reference a
parent's slow writer re-creates after its retirement (its fork recorded long before, by the
parent's deletion) is not retired again once the dead cell's passes are skipped: space only, as
its child is deleted.

**Chains of forks.** A fork of a fork of a fork, each in its own cell (A forks B, B forks C, and so
on): each cell keeps, for the reference of its child, its own branch's whole state at that fork
point, and that state names what the branch inherited where the ancestors' cells keep it. A
reference is retired only once its child's cell is dead, so **every ancestor's fork-point state
stays while any descendant lives**, however deep: A's cell keeps A's state at B's fork point while
B, C or anything after lives. What that holds for a long time is the sum over the chain of each
ancestor's state at its fork point, less what they share (an extent inherited down the chain is
one object), plus each ancestor's history to its fork point that recovering it reads. A lineage
where each generation replaces the last (fork the VM, delete the old one, again) holds every
generation's state at its fork: up to the chain's length times a disk's worth of data that was
overwritten since, where an exact collector would hold one disk. Shallow chains (a base and its
clones, a VM and its snapshots) cost nothing of note; a long replacing lineage is the case to
watch. *Left open:* bounding it would take a child's cell rewriting what it still names of its
inheritance into its own objects, with a checkpoint of its own, and then retiring its reference
early, a copy across cells the model doesn't cover.

**Rules a clone's reference covers too.** With every clone's reference listed after the states,
TLC finds no violation without two older rules: listing the records again after the states
(`recordsRelisted`: the clone, made after the records were read, is found by its reference) and
recovering a branch once by each origin naming it (`recoveriesByOrigins`: a restore's origin is
kept by its reference's fork point). Their mutations now remove `references` with them. A pass
keeps both, which cost one listing of the records and nothing; TASK-395.4 may drop them once
models of more branches agree.

**What it wastes:** what a writable branch's forks freed of what they inherited, until they go, and
the parent's history to the fork point; a live point's clones waste nothing, as the point keeps
that state anyway. *Instead:* forks' cells publishing what they still hold of their parents',
exact, but a protocol across cells every pass.

**For the code, beyond the model** (TASK-395.3, 395.4):

- **Listings are atomic in the model; a LIST is not**, its pages read one after another. A fork
  point is missed if a clone's reference is created behind the listing's cursor while the point's
  own reference ahead of it is retired. So a pass **lists its references twice**, keeping a fork
  point either listing shows: a reference created during the first is in the second whole, and one
  created after the first ended came while the point's reference still stood, throughout the first.
- **The model's recoveries are atomic; the code's are not.** A clone being made whose recovery fails
  is given up on if its point's record, read again, says deleted, rather than failing the pass.
- **Several volumes in a cell:** retirement waits for the whole cell, so a deleted volume of a
  cell forked whole (TASK-364.1) keeps its parent's fork point until the cell goes.

The rest of a pass, unchanged within the cell:

- **Marking.** The objects holding the extents the states name, and the moves the way to them goes
  through, whose maps are needed even once their data moved on. A branch's writer names its
  extents where the checkpoint it recovered from, or its own since, said, which the state the pass
  read may name further along their moves: a checkpoint of another writer's landing after the
  writer recovered the branch, or the parent's state a branch inherited them from, which its writer
  read as it was then. So **the way starts where each extent was sealed**, every moves on it kept,
  unless the state's checkpoint is by the writer holding the branch's lease, read after the
  checkpoint was listed (TASK-175); for a fork point's state, always.
- **Compaction** of objects whose live extents are under a fraction of their data: segments no
  recovery reads, and moves. Their live extents go to the object's moves, data and map in one
  object, written whole, so there is never compaction output without its map, and no orphan of it.
  A reader that finds an object gone reads its moves.
- **Deletion** of what is listed and not needed:
  - segments a checkpoint covers that hold no live extent, or were compacted a grace period ago;
    never a live branch's segments at or past its end, which may be uploaded and not yet in it,
    nor its pins, the positions a writer of it, taken over or taking it over, may still create at
    (`collector::pinned`; Fencing, TASK-371, TASK-388);
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

**The model's bounds** (`scripts/check-tla.sh`; on a laptop, the new models and mutations
under five minutes each, BucketForks and BucketOverlap about 14 minutes, as long before cells,
BucketTakeovers near two hours): the models of before, with a cell each for two branches (`CellEach`, every fork,
point and clone across cells, as hosts make them) and the root's cell shared with branch 2 for
three (`Pair`, forks within a cell and across), passes of any cell; `BucketCells`, a root, its
child and grandchild in three cells, any deleted, three passes of any cells one after another,
retiring down the chain; `BucketCellsPoints`, a point of the root restored to a third cell, point
and root deleted, two passes of any cells at once. Passes of different cells overlap where a
model has "overlap" (BucketOverlap, BucketCellsPoints). Invariant `ReferencesStand`: every live
branch forked from another cell has its reference. The existing rules' mutations keep one cell
(`OneCell`), the whole-bucket pass they were written against.

**Watching before deleting** *(added 2026-11-08, TASK-264).* Two passes delete nothing, for an
operator to see what the collector would do before trusting it with a bucket:

- `fxdax-gc URL --dry-run [--grace S] [--compact-below F]` writes nothing at all. It does not take
  its cell's `<C>gc/lease`, which would bump the epoch and fence a pass running meanwhile out of its deletes; it
  reads the lease's epoch as it starts instead, and says it was fenced if a pass took the lease
  before it ended, what it read being possibly older than that pass's deletes. Nor does it write
  what giving up on forks writes (a fork of a deleted point being made, a failed fork's claim): it
  takes them as given up on. It compacts nothing, and prints each object a pass would delete, then
  each it would compact by its listed size, `KEY KIND SIZE delete|compact`, and their counts and
  bytes. It **undercounts**: what a pass compacts it deletes a grace period after, which a dry run
  never finds doomed. With `--compact-below 0`, a pass deletes exactly what it lists.
- `fxdax-gc URL --mark-only [--every S] [--grace S] [--compact-below F]` is a pass in every respect
  but deleting: it compacts as one does (the compacted originals are the keys most worth watching,
  TASK-175), and instead of deleting writes `<C>gc/marks/<epoch>`, the doomed objects of its cell as listed.
  *Decided:* it **takes `<C>gc/lease`** (step 1). Its marks are ordered by the epoch, each pass's its
  own and later than every pass's before, so readers and the next mark-only pass find the newest by
  key; a pass that deletes, fenced by it, is late, never wrong; and where it finds lost what a live
  branch needs, it can tell a pass that deleted since (it was fenced, and stops, TASK-180) from a
  bug, which a pass not holding the lease could not. Fenced, it writes no marks, as a fenced pass
  deletes nothing. Nothing marked goes, so **storage grows**: originals and their moves both, and
  every marks object, until an operator collects for real or removes them. The collector never
  takes `<C>gc/` for a candidate, nor for a branch's.
  - **Readers warn.** Every reader of a bucket reads it through `store::Store` (extentd's remote
    stores, writers recovering, fxdaxctl, the collector), which holds the newest marks' keys,
    read again every 60 s by a listing of `gc/marks/` (and a read once it changed; per cell from
    format 8, the marks of the cells it reads) alongside one of its reads, uncounted in its stats, never a round trip a read. A GET, ranged GET or HEAD of a
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
rules need a model and tests before code"): `crates/extent_remote`, its `sim` module (TASK-28.1),
since TASK-88 extent_store's (`crates/extent_store/src/sim`), whose hosts' stores are
extent_store's and their uploaders `extent_store::upload`'s, on its own clock.

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
  links between segments, the collector keeping fences and the segments of branches being taken
  over, acknowledging a fork after a lease read, minting from the epoch's range,
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
  prevents, run by `scripts/check-tla.sh` (`make check-tla`): the pins' cases, `pins_fences`,
  `pins_old_fences`, `pins_void_strays`, `pins_late_strays` and `pins_takings` (`NoPinsFences`,
  `NoPinsOldFences`, `NoPinsVoidStrays`, `NoPinsLateStrays`, `NoPinsTakings`; TASK-371, TASK-383,
  TASK-388), which keep the property `CreatesNeverFreed`, and `forks_after_lease` (`NoForksAfterLease`), which took
  `acks_after_lease`'s place (`NoAcksAfterLease`, TASK-371), `fence` (`NoFence`), `continuity` (`NoContinuity`), `epoch_sequences`
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
  TASK-183), `deletions_reread` (`NoDeletionsReread`, TASK-380), and doc-14's `wal_follows` (`NoWalFollows`) and `wal_after_fence`
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
