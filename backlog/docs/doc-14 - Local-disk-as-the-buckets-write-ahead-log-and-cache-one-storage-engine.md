---
id: doc-14
title: 'Local disk as the bucket''s write-ahead log and cache: one storage engine'
type: specification
created_date: '2026-11-05 03:13'
updated_date: '2026-11-05 08:13'
---
# Local disk as the bucket's write-ahead log and cache: one storage engine

Design, 2026-11-04 (TASK-165, its AC #1). The user decided the direction that day
(docs/reviews/2026-11-04-first-principles.md, section 2; decision 2 there), to land after the
copy-on-write DAX work (TASK-164). This doc says what goes, what local disk holds afterwards, the
format changes, how each concern lives in doc-7's protocol alone, the one replay, the
local-filesystem bucket, how the tests carry over and how the risky parts are checked, and the
landable steps (TASK-165.1 to 165.10). Decisions taken here are also in OPEN_QUESTIONS.md under
`# Decisions`.

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

Per store directory (one branch):

| Path | What | Lives until |
|---|---|---|
| `store` | the description: format (6), block size, the branch, the bucket's URL, the placement's fencing token | the store is deleted |
| `open/<n>` | the open segment's data, raw: pieces of open extents and the runs commits sealed | it closes and its framed file is synced |
| `open/<n>.log` | the commits sealed in it: one record per commit (its `Contents`, below), each hashed, appended and synced by the flush that makes the commit durable | as above |
| `open-extents/<sequence>` | large open extents (`Options::large_extent`), raw | sealed |
| `wal/<seq>` | a closed segment, framed, its footer stamped by the writer's chain: the object `segments/<branch>/<seq>` byte for byte | acknowledged, then moved into the part cache |
| `wal/<seq>.<sequence>` | its extents stored on their own, framed: the objects `extents/<branch>/<epoch>.<seq>.<sequence>` | likewise |

Per host (`FXDAX_EXTENTD_CACHE`), shared by every store: `fxdax-parts/<key>`, the part cache
(`extent_remote::cache`), now kept across restarts, and taking whole objects in (`adopt`).

Gone from a store's directory: `branches/`, `segments/`, `extents/`, `index/`, `open-segments/`,
handles naming another store's directory, `store.new`/`store.deleted`, and remote.rs's
`remote-*`, `fence-*` and `origin-*` files (the token moves into `store`).

The host's disk is bounded by the lag bound (`Options::max_lag_bytes`, what the WAL holds: commits
stall there, doc-7 decision 3), the open segment's room (the store's capacity plus what rewrites
before a commit waste, the rule `state::Space` keeps today), and the cache's capacity and floor.
Local dead space no longer exists, so the bound of three times the capacity (TASK-79.89) and local
cleaning go.

## What goes

Lines are code without tests, as of 9f81f6018; rough.

| Module | Lines | Fate |
|---|---|---|
| `extent_store/src/lineage.rs` | 573 | goes: no lineage |
| `extent_store/src/branch_records.rs` | 428 | goes: records are the bucket's |
| `extent_store/src/layers.rs` | 430 | goes: forks share state through `extent_remote::recovery::Layers` |
| `extent_store/src/ledger.rs` | 447 | goes: no file is claimed by several branches |
| `extent_store/src/index.rs` | 564 | goes: no index; the open segment's log replaces deltas, the bucket's checkpoints replace local ones |
| `extent_store/src/host.rs` | 411 | goes but for opening and making a store directory (~100) |
| `extent_store/src/uploads.rs` | 140 | goes: the WAL replaces it |
| `extent_store/src/kept.rs` | 208 | mostly goes (~150): nothing keeps freed data for an uploader; views keep what they pin |
| `extent_store/src/layout.rs` | 546 | ~300 go: `BranchRecord`, `Origin`, the local `Checkpoint`, `Change`, `Delta`, the record codec |
| `extent_store/src/segments.rs` | 1,367 | ~500 go: cleaning (`victims`, `reclaimable`, `place`/`displace`), sharing (`share`, `unshare`, `track`, `Inherited`), `reap`, dead-disk accounting |
| `extent_store/src/state.rs` | 987 | ~400 go: garbage and the 3x bound, kept frees, `Unreferenced`'s file keys |
| `extent_store/src/store.rs` | 2,304 | ~1,100 go: `fork*`, `rebuild`, `clean*`, `adopt`, `spend`, `snapshot`, `upload`, `release`, `read_kept`, lineage plumbing, `make_moves`, index appends |
| `fxdax/src/remote.rs` | 1,681 | ~1,300 go: `Marker`, `InFlight`, `Claim`/`Forked` and their files, `Plan`, `diff`, `take`, `take_parent`, `record`, `start_point`; the uploader loop (~400) moves to extent_store |
| `fxdax/src/extents.rs` | (part) | ~120 go: `fork_durable`'s claims, disclaims and unforks, `copy_bases` stays |

About 5.4k lines of extent_store's code and 1.3k of remote.rs's go; about 1.3k come (the WAL, the
open segment's log, the store directory, the uploader moved, the file bucket, the cache's
persistence). Of the tests, `tests/lineage.rs`, `tests/shuttle/lineage.rs`, `tests/host.rs` and
the lineage, cleaning and uploader cases of `tests/store.rs` (about 2.5k lines) go or are
rewritten against the bucket (Tests, below).

**What stays in extent_store:** the write path (open extents, sealing into one run at a commit,
segments rolling over between commits, framing on a thread of its own: segments.rs, the open half
of state.rs and store.rs), group durability (`durability.rs`), reads of local framed files
(`framed.rs`), the `Dir` abstraction and `MemDir`'s crash simulation, Shuttle's `sync.rs`, views
(`views.rs`), room accounting. It now depends on `extent_remote`.

## The one replay

A segment's footer is `Contents` stamped with the chain's place (`extent_frame::segment`). The
one replay is `extent_remote::recovery::State::apply(key, &Footer)`, split into the continuity
check (doc-7, Fencing: a footer not following the last applied is void) and
`State::apply_contents(key, &Contents)`, which does the rest: seals at their locations, frees,
spends, root, next sequence, capacity, forks. Every reader of a branch's history uses it:

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
the footer's contents, which `Window::commit` computes today in memory. A segment's commits seal
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
5. The branch's state is the recovery's, the WAL's footers applied over it. The guest mints from the
   writer's floor (`Chain::next_sequence`).

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
  stamps its footer with the writer's chain and syncs it into `wal/`, then its raw file and log go.
- **The uploader** (moved from remote.rs into extent_store, `drain.rs`) hands WAL segments to the
  writer (`Writer::add_segment`), uploads them within the host's budget, acknowledges them in order
  after a lease read, and then moves each acknowledged file into the part cache (`Cache::adopt`, a
  rename on the same filesystem), its own extents with it. `COMMIT_DURABLE` completes as now.
  Checkpoints are the writer's, every 64 segments and at forks, as now.
- **Lag:** `wait_lag` and `wait_uploaded` keep their meaning; the bytes the lag counts are the WAL's.

## Reads

An extent's place is one of: in the open segment (raw, its pieces), in a WAL segment (its framed
file), or in the bucket (a `Location`). Bucket places are read by the host's fetcher, from the part
cache, which holds what was acknowledged here whole, and by ranged GETs otherwise
(`extent_remote::fetch`). `Store::read` reads the first two; the third returns the location, which
`crate::cache` fetches as it does for taken stores today (`RemoteData`). Moves the collector made are
followed by the fetcher, as now.

## Forks, points, deletion: only in the bucket

| Operation | Today | Afterwards |
|---|---|---|
| Make a store | local lineage + `branch::create` at attach | `branch::create` (a root), then the directory |
| Open | local index replay, then attach: marker, `Plan`, `diff`, `take` | Opening a store, above |
| Take elsewhere (`fxdaxctl take`) | an empty store adopting every extent, a local checkpoint, a marker | a directory naming the branch; opening it |
| Fork | `Remote::claim` (a GET and a claim file), a local fork (record, layers, handle), then `Remote::forked` waits for the record | a GET of the child's record (taken: `EEXIST`), a commit whose segment names the child (`Footer::forks`), then waiting for its acknowledgement and the child's record (`record_forks`); then the child's directory |
| Fork of a point | a claim, a local fork of the point's copy, `branch::fork_point` | `branch::fork_point`, then the child's directory |
| Point | a fork to a point | a fork to a point (the record says `point`) |
| Open a child | local layers over the parent's index | recovery from the bucket: the parent's state at the fork, shared in memory (`Layers`, the host's `Checkpoints` cache seeded with the writer's checkpoint at the fork) |
| A fork failing | disclaim, `fail_fork`, `unfork` (`delete_branch`), handle deletion | `ChildTaken`: nothing made, nothing to delete. Fenced before the acknowledgement: `branch::delete_fork(child, owner)`, so a segment landing late gives the child a deleted record |
| A host crashing mid-fork | claim files, markers, `take_parent` | the fork is in the WAL; it lands and is recorded by the next writer (the fork stands, as `forks_whose_answers_are_lost_stand` already expects), or the disk is lost and it never was |
| Delete | `delete_host` (claims, handles, lineage), `branch::delete` | `branch::delete` with the token, then the directory |
| Place | `fence-<branch>` | the token in `store` |
| Resize | the local record's capacity | a commit setting the capacity (format change) |
| Clean | `Store::clean_behind` and the collector | the collector |

The child's directory is made once its record exists; opening it is opening a store. Every fork
already waits for its record today (`Remote::forked`), so a fork's latency doesn't change but for
the child's recovery, which, on the fork's host, reads the checkpoint the writer just wrote from
memory.

## Capacity, spent, room

- **Capacity** becomes branch state (`State::capacity`, set by `Contents::capacity`, in
  checkpoints): resizes are ordered with commits, a fork inherits its parent's, a point never
  changes. Today it is in the local record only, so a branch taken elsewhere has the capacity its
  taker gives it.
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
description, which their clones share; their collector runs in-process after commits, as their
cleaning does now (TASK-165.6).

## Format changes and compatibility

- Footers and checkpoints gain the capacity; `layout::FORMAT` becomes 2, and a reader refuses
  objects of other formats, older ones too (`Error::Format`).
- Local stores: a new format (6), described above. Today's stores are not opened, and nothing
  converts them.

No migration: nothing uses fxdax yet (AGENTS.md).

## Tests

**The bucket protocol, once, for every bucket.** A conformance suite in extent_remote
(create-only, conditional update and delete, ranged and suffix reads, sorted listing, `probe`)
runs against `memory:///`, `file:///`, the simulation's `Direct`, and, ignored without their
servers, RustFS and Azurite.

**The simulation** (`extent_remote/src/sim`) is where the protocol's interleavings are searched.
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
create-only segments linked by `after`, acknowledging after a lease read, hosts crashing with or
without their WAL and rebasing it after the fence, forks recorded after their acknowledgement,
failed forks' deletions, points and their restores, placements, and one collector listing first,
compacting segments and moves. Invariants: no acknowledged commit is lost (`NoAckedLost`); every
live branch's history is a path of its own commits, each applied once, holding what they made
(`HistoriesOwn`); no ID is minted twice (`NoIdTwice`); nothing a live branch needs is deleted
(`NothingNeededGone`); and the simulation's other checks (points unwritten, leases current,
deletions only by who may, one making per branch, no pass of the collector lost). Ten models as
designed (`specs/bucket/*.cfg`, each a part of the steps, at 2 hosts, 2 branches, 4 segments
(compaction's 3), 1 collector, points at 1 host and 3 branches) hold; one mutation per `Rules`
field and per WAL rule (`specs/bucket/mutations`, named for the rule, `NoFence` for `fence` and so
on) each gives its counterexample, run by `make check-tla` (`scripts/check-tla.sh`). What it found:
the WAL's rebase by segments (above), and two holes in the collector, TASK-173 (a child recovered
by its parent's state is not recovered again by a record of another origin found on relisting)
and TASK-174 (a deleted root's record dropped, then made again as a fork's child by its parent's
slow writer), which the spec models fixed; and it reproduces TASK-175 (`mutations/Task175.cfg`: a
fenced writer's late checkpoint naming moves, the moves compacted and deleted under the live
writer), which its models as designed leave out (no compaction of moves) until TASK-175's fix is a
rule.

**The seam's tests become tests of the bucket protocol:**

| Today | Count | Afterwards |
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

## Where this meets other work

- TASK-160.8 (extentd's store table): steps 7 to 9 change how extentd makes, takes, places,
  forks and deletes stores (`extentd/stores.rs`); they start once 160.8 has landed.
- TASK-164 (copy-on-write DAX) and its guest work (160.2, 160.4): `fxdax/src/extents.rs` and
  `crate::cache` change in both; this waits for 164, as the task says.
- TASK-157 (fork tests failing under load): `fork_faults.rs` and the extentd fork tests are
  rewritten in step 7.
- TASK-44.5: step 2 is its AC #1; its trace validation (AC #2) and mutants (AC #3) stay its own.
- TASK-160.15: step 3 is its AC #2.
