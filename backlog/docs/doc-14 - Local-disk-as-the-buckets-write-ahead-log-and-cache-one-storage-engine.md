---
id: doc-14
title: 'Local disk as the bucket''s write-ahead log and cache: one storage engine'
type: specification
created_date: '2026-11-05 03:13'
updated_date: '2026-11-12 08:12'
---
# Local disk as the bucket's write-ahead log and cache: one storage engine

Design, 2026-11-04 (TASK-165, its AC #1). The user decided the direction that day
(docs/reviews/2026-11-04-first-principles.md, section 2; decision 2 there), to land after the
copy-on-write DAX work (TASK-164). This doc says what goes, what local disk holds afterwards, the
format changes, how each concern lives in doc-7's protocol alone, the one replay, the
local-filesystem bucket, how the tests carry over and how the risky parts are checked, and the
landable steps (TASK-165.1 to 165.10). Decisions taken here are also in OPEN_QUESTIONS.md under
`# Decisions`.

**Status, 2026-11-07:** implemented. Steps 1 to 9 are on main (TASK-165.9 landed in c9207e5d4);
this doc was brought to the code then (TASK-165.10): what it describes is what main does, and the
*Before* columns say what went. doc-7 describes the host as it is. The decisions taken on the way
are under Decisions, below; the lines removed and added, by module, are in TASK-165's notes and in
What went.

## Why

Since TASK-45 every store has a bucket, yet a host keeps each branch twice:

- `crates/extent_store` (9.8k lines of code, 7.8k of tests in `tests/`) is a whole storage engine:
  a lineage of branches with records, layers shared by forks, a ledger of which branch claims which
  file, an index of checkpoints and deltas, its own cleaning, and `rebuild`;
- `crates/extent_remote` (12.3k with its simulation) is doc-7's protocol: records, leases, forks,
  points, checkpoints, recovery, the collector;
- `crates/fxdax/src/remote.rs` (1.7k of code, 3.8k of tests with `fork_faults.rs` and
  `restore_faults.rs`) keeps the two agreeing, with three files of its own in each store's
  directory (`remote-<branch>`, `fence-<branch>`, `origin-<branch>`).

The seam is where the bugs were: 79.4, 79.5, 79.20, 79.39, 79.71, 79.72, 79.77, 79.78, 79.82,
79.85, 79.95, 129, 150.1 and 150.6. They fall into four classes, each of which the design removes
rather than tests harder:

| Class | Bugs | Why it goes |
|---|---|---|
| Deciding whose a branch's last commit is from a marker file and a local copy | 79.4, 79.20, 79.39, 79.72, 79.95, 129 | No marker: a WAL segment is the object, stamped, durable before its PUT; footers alone tell what landed |
| Local lineage, handles and partial deletions | 79.5, 79.71, 79.77 | No local lineage: a fork is a record in the bucket; a store directory holds one branch |
| Failed forks, claims and unforking | 79.78, 79.82, 79.85 | No claim file: a fork is a commit naming its child; its record follows the acknowledgement, whoever makes it |
| The local copy holding what the branch doesn't (inherited extents uploaded as own; spent kept locally) | 150.1, 150.6 | No local copy: no diff upload, one state, one spending rule |

Also three footer replays (TASK-160.15: remote recovery, local `rebuild`, local delta replay) and
two cleaners (`Store::clean_behind`, the collector's compaction).

## The design in one paragraph

A branch lives in the bucket and nowhere else. A host serving a branch keeps, on local disk, only
what the bucket doesn't have yet and copies of what it does: the **open segment** (the extents the
guest writes, raw, and a log of the commits sealed in it, made durable by the group flush), the
**WAL** (closed segments not yet acknowledged, each the very object it will be in the bucket,
stamped by the writer's chain), and the host's **part cache** (sparse copies of objects' parts,
which a WAL segment joins whole once acknowledged). Opening a store takes the lease, recovers the
branch from the bucket, and puts the WAL after the branch's end; there is no other recovery. Forks,
points, records, checkpoints, deletion and cleaning are doc-7's alone.

## What local disk holds afterwards

Per store directory (one branch, `<b>` its ID; `extent_store::layout`):

| Path | What | Lives until |
|---|---|---|
| `store` | the description (`layout::FORMAT`, local format 11): block size, the branch, as `TENANT/CELL/ID` (TASK-364), the bucket's URL, the placement's fencing token, the capacity a store takes before a commit of its branch sets one, and the volume of its cell it is, by name and kind (`volume`, `kind`), which its opening adds to the cell | the store is deleted |
| `unmade` | a marker `Store::make` writes before anything else, which its maker removes once the store is whole; opening refuses a store holding it (TASK-196) | the store is whole |
| `open-segments/<b>/<n>` | the open segment's data, raw: pieces of open extents and the runs commits sealed | it closes and its framed file is synced |
| `logs/<b>/<n>` | the commits made in it: one record per commit (its `Contents`, below), each hashed, appended and synced by the flush that makes the commit durable, whether the segment is open or being framed | as above |
| `open-extents/<b>/<sequence>` | large open extents (`Options::large_extent`), raw | sealed |
| `segments/<b>/<n>` | a closed segment, framed, numbered locally, its footer stamped by the writer's chain as it closed: the object `segments/<branch>/<seq>` byte for byte, but for a rebase's restamping, which is in memory, the upload restamping the bytes (`extent_frame::restamp`) | acknowledged, then moved into the part cache |
| `extents/<b>/<sequence>` | its extents stored on their own, framed: the objects `extents/<branch>/<epoch>.<seq>.<sequence>` | likewise |
| `wal` | the WAL's base, the commit its first segment was stamped over: two 4 KiB slots, written durably before what an acknowledgement let go of leaves (`wal::BASE`) | the store is deleted |

Beside them, fxdax's own records of the store (`fxdax::store_dir`, TASK-160.13): what is attached
to it, what it was forked from (`ancestry`), the branches marked public (`public-<b>`) and boot
profiles, each written whole and synced. An image's own file bucket is beside its directory,
`<image>.bucket`, which its clones share and which outlives it (Decisions).

Per host (`FXDAX_EXTENTD_CACHE`), shared by every store: `fxdax-parts/<key>`, the part cache
(`extent_remote::cache`), kept across restarts and taking whole objects in (`Cache::adopt`).

Gone from a store's directory: `branches/`, `segments/`, `extents/`, `index/`, `open-segments/`,
handles naming another store's directory, `store.new`/`store.deleted`, and remote.rs's
`remote-*`, `fence-*` and `origin-*` files (the token moves into `store`).

The host's disk is bounded by the lag bound (`Options::max_lag_bytes`, what the WAL holds: commits
stall there, doc-7 decision 3), the open segment's room (the store's capacity plus what rewrites
before a commit waste, the rule `state::Space` keeps), and the cache's capacity and floor.
Local dead space no longer exists, so the bound of three times the capacity (TASK-79.89) and local
cleaning go.

*Implemented 2026-11-06 (TASK-165.9), differing from the design in names, not in kind:* the
design's `open/<n>`, `open/<n>.log` and `wal/<seq>` are the table's `open-segments/`, `logs/` and
`segments/`, kept per branch; the design had no `unmade`, nor the WAL's base in a file of its own.
OPEN_QUESTIONS.md says why each.

## What went

Lines are code without unit tests (a file's lines before its `#[cfg(test)]` module), at 9f81f6018,
where the design counted them, and on main at 213504c4b, after step 9. The estimate is the
design's. Other work changed some of these files meanwhile (TASK-160's refactors, TASK-149's FUSE,
TASK-154's tenants), counted in the second figure too; TASK-165's notes have the counts of the
TASK-165.1-165.9 commits alone.

| Module | 9f81f6018 | Estimate | main | What happened |
|---|---|---|---|---|
| `extent_store/src/lineage.rs` | 574 | goes | gone | no lineage |
| `extent_store/src/branch_records.rs` | 428 | goes | gone | records are the bucket's |
| `extent_store/src/layers.rs` | 430 | goes | gone | a child recovers its state from the bucket (`extent_remote::recovery::Layers` within one recovery) |
| `extent_store/src/ledger.rs` | 447 | goes | gone | no file is claimed by several branches |
| `extent_store/src/index.rs` | 564 | goes | gone | no index: the open segment's log replaces deltas, the bucket's checkpoints local ones |
| `extent_store/src/host.rs` | 411 | ~100 stay | 237 | making, opening, placing and deleting a store directory, `unmade` (TASK-196) |
| `extent_store/src/uploads.rs` | 141 | goes | 55 | a closed segment for the uploader (`Closed`) |
| `extent_store/src/kept.rs` | 208 | ~150 go | 219 | stays: frees are kept until acknowledged, so that a view or a rebase reads them (Decisions) |
| `extent_store/src/layout.rs` | 546 | ~300 go | 381 | 165 went: `BranchRecord`, `Origin`, the local `Checkpoint`, `Change`, `Delta`, the record codec; the commit log's records came |
| `extent_store/src/segments.rs` | 1,367 | ~500 go | 979 | 388 went: cleaning, sharing, dead-disk accounting |
| `extent_store/src/state.rs` | 988 | ~400 go | 1,109 | garbage, the 3x bound and `Unreferenced`'s file keys went; one replay over the branch's state, `Life` and `Standing` (TASK-160.18, 160.19) came |
| `extent_store/src/store.rs` | 2,304 | ~1,100 go | 2,316 | `fork*`, `rebuild`, `clean*`, `adopt`, `spend`, `snapshot`, `upload`, `release`, lineage plumbing went; opening by recovery, `rebase`, the WAL's leaving into the cache, framings as one queue and flush timings came |
| `extent_store/src/wal.rs`, `upload.rs`, `timings.rs` | 0 | ~1.3k come, with the file bucket and the cache | 469, 1,112, 160 | the WAL's recovery and base; the uploader, moved from remote.rs; flush timings |
| `fxdax/src/remote.rs` | 1,687 | ~1,300 go, ~400 move | 178 | 1,509 went: `Marker`, `InFlight`, `Claim`/`Forked` and their files, `Plan`, `diff`, `take`, `take_parent`, `record`, `start_point`; the uploader moved to `extent_store::upload` |
| `fxdax/src/extents.rs` | (part) | ~120 go | | `fork_durable`'s claims, disclaims and unforks went, `fork_durable` now a call of `host_store::fork`; `copy_bases` went too (TASK-165.7): what is attached to a store stays attached to its fork by what the fork's directory inherits (`store_dir::inherit`) |
| `fxdax/src/host_store.rs` | 191 | | 443 | every store's bucket, forks in the bucket, `unmade` |
| `extent_remote` `local.rs`, `wal.rs`, `cache.rs` | 0, 0, 316 | | 212, 147, 905 | the file bucket; `Wal::rebase`; the part cache kept across restarts and adopting objects |

extent_store's code went from 10,757 lines to 9,580 (1,177 fewer; the design's estimate was 4.1k
fewer, 5.4k going and 1.3k coming), its `tests/` from 7,779 to 5,930; remote.rs from 1,687 to 178.
extent_store lost less than estimated because what replaced the index and the lineage (one
recovery from the bucket and the WAL, framings as one queue, crash-tested opening) is larger than
the design guessed, and kept frees stayed. Of the tests, `tests/lineage.rs` and
`tests/shuttle/lineage.rs` went; `tests/store.rs`' lineage, cleaning and uploader cases went or
became tests against the bucket (Tests, below).

**What stays in extent_store:** the write path (open extents, sealing into one run at a commit,
segments rolling over between commits, framing on a thread of its own: segments.rs, the open half
of state.rs and store.rs), group durability (`durability.rs`), reads of local framed files
(`framed.rs`), the `Dir` abstraction and `MemDir`'s crash simulation, Shuttle's `sync.rs`, views
(`views.rs`), room accounting. It now depends on `extent_remote`, and holds the uploader
(`upload.rs`) and the WAL (`wal.rs`).

## The one replay

A segment's footer is `Contents` stamped with the chain's place (`extent_frame::segment`). The
one replay is `extent_remote::recovery::State::apply(&mut self, &Footer) -> bool`, split into the
continuity check (doc-7, Fencing: a footer not following the last applied is void, and `apply`
returns false) and `State::apply_contents(&mut self, seq, epoch, &Contents)`, which does the rest,
the segment's number and its writer's epoch placing what it seals: seals at their locations,
frees, spends, root, next sequence, capacity, forks. Every reader of a branch's history uses it:

- recovery of a branch (`recover`, `recover_on`, `state_at`), and so the writer, the collector,
  `branch::delete`, points;
- the writer, applying what it acknowledges;
- extent_store's live state: a commit applies its contents to the branch's state at the open
  segment's provisional key, so that the state after a segment closes is the replay of its footer
  over the state before it, which the `audit` feature asserts at every close;
- opening a store: the bucket's recovery, then the WAL's footers, then the open segment's log
  folded into the contents of its footer.

Each of TASK-160.15's differences gets one rule:

- **Spending:** a footer frees only extents a commit held (`layout::held_frees`, already so), and
  replay spends every free of the branch's own extents. Remote recovery's "held before the segment"
  rule goes: it existed to leave an ID sealed and freed within one segment writable elsewhere, which
  nothing needs, as each writer mints from its own epoch's range.
- **Next sequence:** the largest named, as remote recovery has it.
- **Continuity:** the footer's `after` link (remote's). Local rebuild's anchoring on its
  checkpoint's segment goes with local checkpoints.
- **Checkpoints:** one schema, `extent_remote::checkpoint`'s.

The open segment's log is not a second replay: its records are each commit's `Contents`, and
folding them (concatenating lists, the last root, the largest next sequence, the last capacity) is
the footer's contents, which the open segment's `Window` computes in memory as commits are made. A segment's commits seal
each extent once and free it at most once after, so applying the folded contents equals applying
the commits in turn.

## Opening a store

1. Read `store`: the branch, the bucket, the token.
2. **Take the lease** with the token (`Writer::open`): recover the branch, fence its end. A point
   takes none: its state is recovered and never changes.
3. **Close the open segment** if one was left: its data and the whole records of its log (up to the
   first torn one) give its contents; it is framed from its raw file, as `close_left_open` does now,
   and stamped by the new chain. Writes after the last durable commit are dropped, as now.
4. **Put the WAL after the branch's end** (`extent_remote::wal::rebase`, new), by commits, as the
   branch is after the fence (`wal_after_fence`: the fence may have applied the WAL's own segments,
   landed since recovery found the end). The WAL keeps its **base**, the commit its first segment
   was stamped over (the branch's last commit then), rewritten as its prefix is acknowledged:
   - if the branch's last commit is one of the WAL's, the WAL's segments to it are in the branch:
     acknowledged now, dropped from the WAL, into the cache;
   - if the branch's last commit is that one, or the WAL's base, the rest are restamped on the new
     chain after the fence (`wal_follows`) (their data parts unchanged; a new tail; their own
     extents at the new writer's keys), and upload as any closed segment;
   - otherwise another writer's commits moved the branch on: the WAL **diverged**, the store refuses to open
     (`extent_remote::Error::Diverged`, `ESTALE`) until `fxdaxctl take` drops its WAL, losing what
     the guest was never promised (doc-7, Recovery on the same host). An empty WAL never diverges:
     opening a store directory of an empty WAL is how a branch is taken, here or on any host.
   Restamping writes each new file and syncs it before removing the old; a WAL holding both after a
   crash keeps the one stamped last, by their footers' epochs.

   *Changed 2026-11-05 (TASK-165.2), found by the TLA+ spec (model BucketWal's bounds with
   checkpoints):* this step went by segments first: the WAL segments whose footers recovery read
   were acknowledged, and the first other one had to follow the fence's `after`. Recovery reads no
   footer a checkpoint covers, so a WAL whose segments landed and a later writer then checkpointed
   past them was declared diverged though the branch holds every one of its commits: host A
   uploads segment 1 and crashes keeping its WAL, host B takes the branch, recovers segment 1,
   fences and checkpoints, host A takes the branch back, recovers from B's checkpoint, and refuses
   to open. So was a WAL the branch moved past by an empty fence alone (a takeover that crashed
   after fencing), its guest's commits dropped by `take` though nothing came after them. By commits,
   as doc-7's recovery on the same host has it ("if it holds the local ones up to there"), neither
   diverges, and what decides is what the guest's history is. A segment holding no commit but a
   fork stamps the base it was stamped over, as one with commits does.

   *Implemented 2026-11-05 (TASK-165.5):* `extent_remote::Wal::rebase` takes what the writer found
   at the branch's end (`Writer::fenced`: its last commit as recovery found it and after the fence,
   and its chain) and places the WAL's segments by the guest's last commit through each, its base
   for none: a segment holding no commit, a fork's alone, is placed by the commit before it, and of
   the places the branch's last commit matches the last is taken as landed (a fork whose segment
   did not land, never acknowledged, is lost, rather than made twice). The WAL's base is then the
   branch's last commit after the fence.
5. The branch's state is the recovery's, the WAL's footers applied over it. The guest mints from the
   writer's floor (`Remote::next_sequence`, the writer's epoch's range). The store's next sequence
   on opening is one past the highest of its own extents it holds, where fxfs looks for orphans
   below (`Allocator::open`), not the footers' stamp, which their writers stamped past their
   floors.

What made a stale store refuse before, the marker naming which commits were its own, is the WAL
itself: every segment that may land is a WAL file, stamped and synced before its PUT (TASK-79.72's
rule), and nothing else can land in the writer's name.

## Commits, closing, uploads

- **A commit** seals the extents written before it into the open segment, in one run each, or into
  files of their own if large (as now), and applies its contents to the branch's state. **A flush**
  syncs the open segment's data and own files, then appends the commits' records to the log and
  syncs it: the commits are durable locally (doc-7, Uploads, step 2). No index.
- **The open segment closes** when full, at its age (the upload interval), when asked (a sync, a
  fork, a commit waiting to be durable, stopping), as now; its framing, on a thread of its own,
  stamps its footer with the writer's chain and syncs it into `segments/`, then its raw file and log
  go.
- **The uploader** (moved from remote.rs into extent_store, `upload.rs`) hands WAL segments to the
  writer (`Writer::add_segment`), uploads them within the host's budget, acknowledges them in order
  (after a lease read; since TASK-371 only a fork's, the creates the fencing, doc-7), and then
  moves each acknowledged file into the part cache (`Cache::adopt`, a
  rename on the same filesystem; `store.rs`'s `leave`), its own extents with it, or removes it if
  the cache can't take it. `COMMIT_DURABLE` completes as now. Checkpoints are the writer's, every
  64 segments (`upload::CHECKPOINT_EVERY`), on clean shutdown, and as an acknowledgement carries
  forks: the checkpoint makes them.
- **Lag:** `wait_lag` and `wait_uploaded` keep their meaning; the bytes the lag counts are the WAL's.

*Implemented 2026-11-06 (TASK-165.9):* the uploader is `extent_store::upload`; framings are one
queue, in closing order; a segment goes to the uploader once framed and its commits durable; a
capacity change is a commit of its own; recovery keeps the WAL's whole framed segments to the first
not whole whose log holds commits, which it closes again, those after it holding no durable commit
going first, durably (found by the crash test cutting openings at each write); an opening whose WAL
base names commits the bucket acknowledged, where the bucket has no such branch, refuses
(`NoBranch`, `ENOENT`). `COMMIT_DURABLE` still waits for the upload and its acknowledgement, as
above: the real-GCS fdatasync latency (TASK-165.11) is that wait, which this does not change.

## Reads

An extent's place is one of: in the open segment (raw, its pieces), in a WAL segment (its framed
file), or in the bucket (a `Location`). Bucket places are read by the host's fetcher, from the part
cache, which holds what was acknowledged here whole, and by ranged GETs otherwise
(`extent_remote::fetch`). `Store::read` reads the first two; the third returns the location, which
`crate::cache` fetches (`extent_store::RemoteData`). Moves the collector made are followed by the
fetcher.

## Forks, points, deletion: only in the bucket

| Operation | Before (went) | Now |
|---|---|---|
| Make a store | local lineage + `branch::create` at attach | `branch::create` (a root), then the directory |
| Open | local index replay, then attach: marker, `Plan`, `diff`, `take` | Opening a store, above |
| Take elsewhere (`fxdaxctl take`) | an empty store adopting every extent, a local checkpoint, a marker | a directory naming the branch; opening it |
| Fork | `Remote::claim` (a GET and a claim file), a local fork (record, layers, handle), then `Remote::forked` waits for the record | a GET of the child's record (taken: `EEXIST`, `Remote::child_free`), a commit whose segment names the child (`Store::commit_fork`, `Footer::forks`), then waiting for its acknowledgement and the child's record (`Remote::forked`, the writer's `record_forks`); then the child's directory (`host_store::fork`) |
| Fork of a point | a claim, a local fork of the point's copy, `branch::fork_point` | `branch::fork_point`, then the child's directory |
| Point | a fork to a point | a fork to a point (the record says `point`) |
| Open a child | local layers over the parent's index | opening a store: recovery of the child from the bucket, the parent's state at the fork read from the parent's checkpoint and footers, then flattened into the child's own state, O(extents) (Decisions) |
| A fork failing | disclaim, `fail_fork`, `unfork` (`delete_branch`), handle deletion | `ChildTaken`: nothing made, nothing to delete. Failing once its commit was made (its upload or the child's directory failing): `Remote::unfork`, which stays, deleting the fork's alone, `branch::delete_fork(bucket, child, forked, owner, holder, token)`, so a segment landing late gives the child a deleted record; the directory goes |
| A host crashing mid-fork | claim files, markers, `take_parent` | the fork is in the WAL; it lands and is recorded by the next writer (the fork stands, as `forks_whose_answers_are_lost_stand` already expects), or the disk is lost and it never was |
| Delete | `delete_host` (claims, handles, lineage), `branch::delete` | `branch::delete` with the token, then the directory |
| Place | `fence-<branch>` | the token in `store` |
| Resize | the local record's capacity | a commit setting the capacity (format change) |
| Clean | `Store::clean_behind` and the collector | the collector |

The child's directory is made once its record exists; opening it is opening a store. Every fork
waited for its record before too (`Remote::forked`), so a fork's latency changes by the child's
recovery: its state is read from the bucket, the checkpoint the parent's writer wrote as the fork
was acknowledged and the footers after it, and taken by the child, O(extents) of the parent at the
fork, not shared in memory with the parent's. Measured (TASK-165.10,
docs/measurements/restructure-before-after-2026-11-07.md): forks 12 ms against 18 ms before the
restructure and 26 ms before step 9, opens 7.6 ms against 19.4; a cold host's first read of a
fork's data slower, fetched where it was read from local files (TASK-225).

## Capacity, spent, room

- **Capacity** becomes branch state (`State::capacity`, set by `Contents::capacity`, in
  checkpoints): resizes are ordered with commits, each a commit of its own, a fork inherits its
  parent's, a point never changes. It was in the local record only, so a branch taken elsewhere
  had the capacity its taker gave it.
- **Spent** IDs are the replay's, one rule (above): TASK-150.6's local-only spent set goes.
- **Room:** a write is refused (`NoSpace`) when the branch's live bytes and its uncommitted writes
  would pass its capacity, and what rewrites before a commit waste counts once past a capacity's
  worth, as now. The bound of files on disk goes (above).

## Views

Followers are frozen (decision 4 of the review) but kept working: a view pins the branch's state
as of a commit (an `Arc`) and the WAL files it names until it advances. Extents freed after its
commit are read where its state says, in the WAL or the bucket; the collector may reclaim one a
grace period after its free, which cuts the view off (`CutOff`), as lagging too long does now.

## The local-filesystem bucket

`file:///dir` (TASK-165.1, `extent_remote::local`) is a bucket on the host's filesystem, for
development, tests and images. It is `object_store`'s `LocalFileSystem` (fsync on) with conditional
updates added: `LocalFileSystem` refuses `PutMode::Update`. What replaces or removes an object (an
update, an overwrite, a delete) holds the bucket's lock, a `flock` of `.fxdax-lock` in its
directory, each holding opening the file anew, so that it excludes other processes and other
holdings in its own; an update compares the object's e-tag with the one given (`LocalFileSystem`'s:
inode, modification time and size), and writes by rename. Creates are `LocalFileSystem`'s, a hard link, which fails where an object is.
`Store::from_url` takes it like any URL, so stats, probing and the latency throttle apply.

**Images** (`Storage`, the fxdax CLI's) keep their branches in a file bucket named in their
description, `<image>.bucket` beside the image, which their clones share and which outlives it;
their collector runs in-process after commits (`storage::collect`, TASK-165.6), only for an
image's own bucket.

## Format changes and compatibility

- Footers and checkpoints gain the capacity; `layout::FORMAT` becomes 2, and a reader refuses
  objects of other formats, older ones too (`Error::Format`).
- Local stores: a new format, 6 at TASK-165.6, 7 at TASK-165.7 and 8 at TASK-165.9, described
  above (`extent_store::layout::FORMAT`, "fxdax extent store 10", whose footers and logs'
  records name the runs of a commit spanning segments in each it spans, TASK-92.41, and whose
  logs' headers name their segments, TASK-341). Stores of
  another format are not opened, and nothing converts them.

No migration: nothing uses fxdax yet (AGENTS.md).

## Tests

**The bucket protocol, once, for every bucket.** A conformance suite in extent_remote
(create-only, conditional update and delete, ranged and suffix reads, sorted listing, `probe`)
runs against `memory:///`, `file:///`, the simulation's `Direct`, and, ignored without their
servers, RustFS and Azurite.

**The simulation** (`extent_remote/src/sim`, since TASK-88 `extent_store/src/sim`, its hosts'
stores extent_store's over directories in memory and their uploaders the store's own, run on the
simulation's clock) is where the protocol's interleavings are searched.
Its hosts' disks become WALs of stamped segments, and every open on a host with one runs the
production `wal::rebase`, not the simulation's own re-batching (`World::opened`): the host's side
of the seam is then in the simulation. Two rules join `Rules`, each shown needed by a seed:
`wal_follows` (a WAL is put after the branch only if the branch's last commit is one of the WAL's
or its base; specs/bucket's mutation `NoWalFollows`: a host's WAL put after another host's
acknowledged commit, which is lost) and `wal_after_fence` (that last commit is the branch's after
the fence, not as recovery found it; `NoWalAfterFence`: the WAL's own segment, landed between,
applied twice). A crash keeps the WAL or loses it, as `disk` is kept or cleared now.

**Shuttle** (`extent_store/tests/shuttle`) keeps the write path's scenarios (writers, commits,
rolling over, framing, flushes failing) and gains the WAL's: the uploader acknowledging while
commits and closes race, a segment closing for room as one acknowledges, a view pinning a WAL file
being acknowledged. The lineage scenarios go with the lineage. Its uploader drives the WAL's
interface (`take`, `acknowledged`) directly, so Shuttle schedules the store's threads, and the
protocol's interleavings stay the simulation's.

**Crash rounds** (`tests/crash`, `MemDir`) keep their oracle, a prefix of the commits at least as
late as the last durable, now across: a crash with the disk (WAL replayed) and without it (the
bucket's prefix, at least as late as the last acknowledged), reopening against a memory bucket.

**TLA+** (`specs/bucket/Bucket.tla`, TASK-165.2, the task's AC #3; TASK-44.5's AC #1), written
before the code that changes the protocol: leases with fencing tokens, the fence, dense
create-only segments linked by `after`, acknowledging after a lease read (since TASK-371, the
creates the fencing, the collector keeping fences, and a fork's after a lease read), hosts crashing with or
without their WAL and rebasing it after the fence, forks recorded after their acknowledgement,
failed forks' deletions, points and their restores, placements, and passes of the collector,
overlapping, listing first, compacting segments and moves. Invariants: no acknowledged commit is
lost (`NoAckedLost`); every live branch's history is a path of its own commits, each applied once,
holding what they made (`HistoriesOwn`); no ID is minted twice (`NoIdTwice`); nothing a live
branch needs is deleted (`NothingNeededGone`); and the simulation's other checks (points
unwritten, leases current, deletions only by who may, one making per branch, no pass of the
collector lost). Eleven models as designed (`specs/bucket/*.cfg`, each a part of the steps, at 2
hosts, 2 branches, 4 segments (compaction's 3), 1 collector, points at 1 host and 3 branches,
overlapping passes at 2) hold; one mutation per `Rules` field, per WAL rule and per rule the spec
adds (`specs/bucket/mutations`, named for the rule, `NoFence` for `fence` and so on) each gives its
counterexample, run by `make check-tla` (`scripts/check-tla.sh`). What it found: the WAL's rebase
by segments (above), and two holes in the collector, TASK-173 (a child recovered by its parent's
state is not recovered again by a record of another origin found on relisting) and TASK-174 (a
deleted root's record dropped, then made again as a fork's child by its parent's slow writer),
which the spec models fixed; it reproduced TASK-175 (a fenced writer's late checkpoint naming
moves, the moves compacted and deleted under the live writer), whose trace is now
`NoWaysFromSeals`, the mutation of its fix's rule; and it found TASK-183 (two deletions of a
branch at once, the second recording a never-acknowledged fork's child after a pass let go of its
origin), which it models fixed (`deletionsFence`: a deletion fences the end it recovered to).

**The seam's tests became tests of the bucket protocol:**

| Before | Count | Now |
|---|---|---|
| remote.rs, uploads (segments are the files, batches, gathers, flipped bytes, lag, stalls) | 17 | extent_store tests over a memory bucket, unchanged in what they assert; `attaches_upload_what_the_branch_lacks_in_batches` becomes a WAL caught up after an outage |
| remote.rs, markers (`markers_name_*`, `segments_uploading_are_marked_*`, `*_marker_was_lost_*`) | 5 | the WAL's: a segment is a synced WAL file before its PUT; reopening with segments in flight, landed or not, at each point (`wal::rebase` unit tests and the simulation) |
| remote.rs, fencing, placement, formats, host loss, takes | 14 | kept, through opening a store; TASK-79.4's race becomes "the bucket moved on between recovery and the lease" against `wal_follows` |
| remote.rs, forks: records lost, segments landing late, parents taken (TASK-79.20, 79.72) | 10 | the fork recorded by the parent's next writer, or never made; in the simulation's `crashes_anywhere`, and two extent_store tests with a crash after the fork's commit |
| remote.rs, failed forks and claims (TASK-79.78, 79.82) | 12 | `ChildTaken` makes nothing; a fenced fork's `delete_fork`; twins (the simulation's `failed_forks_deleted_anywhere`) |
| `fork_faults.rs` (14), `restore_faults.rs` (8) | 22 | kept: each step of a fork and a restore failing, its host dying, the collector meeting each write; the steps are fewer (no claim, no handle) |
| extentd tests through a bucket | 22 | kept, through the daemon's API; `stores_are_cleaned_behind_their_guests_commits` and `..._once_the_forks_sharing_*` become the collector compacting what a guest freed |
| extent_store, lineage (`tests/lineage.rs`, shuttle lineage, `forks_*`, `deleting_*`) | ~40 | go; what they assert of forks (constant time, sharing, independence) is asserted of bucket forks in extent_store's tests |
| extent_store, cleaning | ~20 | go; the collector's compaction tests stay |

No test is deleted without its replacement in the same step; the user's rule on deleting tests
holds, and each step's notes list what each removed test became.

## Steps

Each lands green (macOS `scripts/test.sh` with `workspace shuttle`, Linux in Docker or GCE), with
its own tests. 1 to 5 change nothing a guest sees and don't wait for TASK-164; 6 to 9 do, and touch
`fxdax/src/extents.rs` and `extentd/stores.rs`, so they start after TASK-164 and TASK-160.8 land.

1. **The local-filesystem bucket** (TASK-165.1): `file://`, the bucket conformance suite.
2. **TLA+ of doc-7 with the WAL** (TASK-165.2).
3. **One replay; capacity in the protocol** (TASK-165.3): `apply_contents`, one spending rule,
   capacity in footers and checkpoints, format 2; extent_store's `rebuild` replays with it
   (TASK-160.15's AC #2).
4. **The part cache kept and adopting objects** (TASK-165.4).
5. **The WAL's protocol** (TASK-165.5): `wal::rebase`, `Error::Diverged`, the simulation's hosts
   keeping WALs, the two new rules.
6. **Images over a file bucket** (TASK-165.6): every store has a bucket, images too.
7. **Forks, points and deletion only in the bucket** (TASK-165.7): lineage, layers, ledger, branch
   records, handles, claims go; a child is opened from the bucket.
8. **Uploaded data leaves the store** (TASK-165.8): acknowledged segments move into the part cache;
   local cleaning and the 3x bound go. Lands in one batch with 7, so that main never has forks on
   one host reading their base back from the bucket.
9. **One recovery** (TASK-165.9): the index goes; the open segment's log; opening by lease, recovery
   and the WAL; markers, `Plan`, `diff`, `adopt` go; the uploader moves into extent_store.
10. **Measured and written down** (TASK-165.10): doc-7 rewritten for what is now, the perf gate on
    GCE against main before, and the lines removed counted.

Out of scope: the serial commit log over parallel data (a SlateDB-style manifest, review section
2), which this makes smaller to do; merging small objects in the collector.

## Decisions

Taken on the way, each in OPEN_QUESTIONS.md under `# Decisions` with what else was possible:

- **A child takes its state in O(extents), not shared in memory** (TASK-165.7): a fork's child's
  store is made once its record exists, and opening it recovers its state from the bucket and
  flattens it, rather than sharing the parent's state in memory as layers did. A fork of a base
  with many extents pays for them once, at the child's opening; nothing ties the child's state to
  its parent's after.
- **`Record::Acknowledged`** (TASK-165.8): acknowledged segments leave the store for the host's part
  cache, which the index recorded (`Record::Acknowledged`) until TASK-165.9 removed the index; the
  WAL's base (`wal`) records it since, written durably before an acknowledged file leaves. A store
  with no uploader kept everything; since TASK-165.9 every store opened has one.
- **`<image>.bucket`** (TASK-165.6, 165.7): an image's own file bucket is beside its directory,
  named for it, not inside it, as its clones keep their branches there and outlive it; it is never
  removed with the image. A directory with no description is no store.
- **`Remote::unfork` stays:** a fork failing after its commit was made deletes the fork's child in
  the bucket, the fork's alone (`branch::delete_fork`), so that its segment, landing late, makes
  nothing (TASK-79.78).
- **One recovery** (TASK-165.9, local format 8): see OPEN_QUESTIONS.md's line, which lists each of
  its choices (the open segment's log, the WAL's base, framings as one queue, frees kept until
  acknowledged, the next sequence on opening, a resize a commit of its own).

**The user accepted TASK-165.9's trade-offs:**

- *2026-11-07:* TASK-165.9 lands with its local-durability lag's tail above main's (pgbench's
  maximum 1.0-2.0 s against main's 0.41-0.77 s; the daemon-killed case's 165-288 ms against
  60-81 ms): its fixes (TASK-195, 204, 196, 192, 208), the one recovery path and the crash tests'
  findings are worth more. The tail is TASK-221's, before TASK-224's fsync waiting for local
  durability is worth enabling.
- *2026-11-06:* doc-12's "durable within 50 ms" becomes a 50 ms median, which the pgbench cases
  check (`durable_lag_within_50ms`), and a separate bound on what a daemon crash may lose, 500 ms,
  which the daemon-killed crash test checks (`CRASH_LOSS_BOUND_MS`, TASK-221); the worst tail is
  performance work.

## Where this meets other work

- TASK-160.8 (extentd's store table): steps 7 to 9 change how extentd makes, takes, places,
  forks and deletes stores (`extentd/stores.rs`); they start once 160.8 has landed.
- TASK-164 (copy-on-write DAX) and its guest work (160.2, 160.4): `fxdax/src/extents.rs` and
  `crate::cache` change in both; this waits for 164, as the task says.
- TASK-157 (fork tests failing under load): `fork_faults.rs` and the extentd fork tests are
  rewritten in step 7.
- TASK-44.5: step 2 is its AC #1; its trace validation (AC #2) and mutants (AC #3) stay its own.
- TASK-160.15: step 3 is its AC #2.
