---
id: doc-8
title: Live read-only followers of a mounted Fxfs
type: specification
created_date: '2026-10-17 22:17'
updated_date: '2026-10-30 22:45'
---
# Live read-only followers of a mounted Fxfs

Status: planned, 2026-10-17; the protocol's TLA+ spec written and checked (TASK-42.8, testing 0);
store views built (TASK-42.2, 2026-10-18); the simulation (TASK-42.1), the follower in Fxfs
(TASK-42.4), its protocol under Shuttle (TASK-42.5, the host's side) and extentd's followers
(TASK-42.3, but Firecracker) built, 2026-10-18; the fill barrier in place of the seqcount, its
mutex fallback and SRCU (TASK-42.10), 2026-10-19. Tasks: TASK-42, in order: the protocol in TLA+
(42.8), store views (42.2), the test foundation (42.1: simulation on Shuttle, history checker,
per-batch comparison, trace validation), the follower in Fxfs (42.4), the concurrency protocol
under Shuttle (42.5, once 42.8 is done), extentd, doc-4 and Firecracker (42.3), the guest kernel
(42.6), the guest torture test and measurements (42.7).

Other VMs on the host mount a store read-only while a VM has it mounted read-write, and follow
it: they see what the writer commits, as it commits it, without remounting. This is the design
agreed with the user on 2026-10-17, and, above all, how it is tested: the hard part of a
follower is concurrency, whose bugs are silent (a stale read crashes nothing), so the plan is
built around testing techniques that make them loud and reproducible.

## Decisions

1. **Live follow, only.** No point-in-time-only stage ships. A follower mounts the store as of a
   commit, then applies each commit after it. (Point-in-time mounts fall out of it: a follower
   that never advances.)
2. **Readers are VMs on the writer's host**, served by the same extentd, so a follower reads
   the writer's sealed extents, and its open extents' sealed parts, from the same memory and
   cache, with no copy.
3. **The writer never waits** for a follower: no commit is forced, no free blocks on one. A
   follower takes the latest commit the writer made, ordered or durable.
4. **A follower sees a prefix of the writer's history**: at every moment, the state as of some
   commit, moving forward. It may see an ordered commit a crash of extentd would lose; followers
   are ephemeral, and die with that extentd anyway.
5. **DAX from the start**: it is the primary path. Without DAX (the page cache) comes along, as
   it shares everything but the invalidation of mappings.
6. **A file the writer deletes while a follower has it open stays readable until the follower
   closes it** (POSIX), through a follower-side graveyard (below), rather than failing with
   ESTALE.
7. **A follower that falls too far behind** (a lag in time, or in bytes the writer freed that it
   pins) **is cut off**: its operations fail with ESTALE until it remounts. That bounds what
   followers cost the writer's space. Limits to be chosen with measurements. A batch that fails
   ends a follower the same way: what it applied can't be taken back, so its readers fail with
   ESTALE until it remounts, on the commit it has kept.
8. **Readers wait for a batch, briefly** (TASK-42.10, the user's decision of 2026-10-19,
   replacing "no global lock stalls a follower's readers"): a fill barrier keeps them out while a
   batch drops what the kernel caches of what it changes and applies it, as no lock-free scheme
   kept their caches whole across batches. See Concurrency.

## Why it works: commits seal

A commit seals every open extent (doc-4): what the writer wrote before it is immutable after it.
The store as of commit N is its extent table and root at N, all of it sealed, and the journal up
to N's sealed lengths. So a follower needs no fork and no forced commit: it needs the writer's
frees after N kept while it may still read what they freed. ExtentStore already keeps freed
extents for one other party, the uploader (`uploads.kept`, until its epoch is uploaded); a view
is a second kind of party.

Fxfs's journal replay ends at the first checksum mismatch (journal/reader.rs). A view returns
nothing past commit N's sealed lengths, so a follower's replay ends exactly at commit N.

## Design

### Store views (crates/extent_store)

Built (TASK-42.2): `crates/extent_store/src/views.rs`, `Store::{view, read_view, locate_view}`.

- `Store::view()`: the branch as of its latest commit, its epoch and root. `View::advance()` moves
  it to the latest commit. Sealed extents never change and IDs are never reused, so a view copies
  no extent table: an extent its commit holds is held still, as it was, wherever cleaning moved
  it, or freed since and kept. A view refuses open extents, which no commit holds; extents of
  later commits it doesn't tell from its own, as its follower never asks for them.
- A free of an extent sealed before is kept while a view older than the commit that freed it
  lives, or a view pins it, or the uploader needs it: not deleted, its segment pieces still
  counted live (so compaction leaves them). The kept bytes count as used (INFO, writes' room).
- What a commit's frees keep is decided as the commit is published to views, in one critical
  section: a view opened before is of an earlier commit, one opened after needs none of them.
  Until its commit, a free is undecided, and its data stays.
- Opening, advancing, pinning and dropping a view take the views' lock alone, never the store's
  state or commit lock: a follower never waits for the writer.
- Pins of single extents beyond the view's epoch (`View::pin`, the follower's graveyard, below).
- Views live as long as their holder. A view stale for `Options::view_lag`, or keeping more than
  `view_kept_bytes` of frees as the oldest, is cut off (decision 7): it keeps nothing, and its
  reads, advances and pins fail with `CutOff`. The limits are to be chosen with measurements.

### extentd and the device protocol (doc-4)

Built (TASK-42.3): `ExtentStore::{follow, advance, let_go, pin, unpin, newer, take_over}`
(crates/fxdax/src/extents.rs), extentd's read-only connections (`extentd/{server, stores,
client}.rs`), doc-4's Followers.

- A follower is an `ExtentStore` of its own (`ExtentStore::follow`), sharing its writer's store
  and the extent cache, with a window of its own and a view: it reads and maps through the view,
  refusing what is outside it and its pins with EIO, logged, and what would change the store with
  EROFS. `advance` keeps the commits between until `let_go` (`View::advance_keeping`), as the
  guest's follower reads them while it applies them, and its readers may still read them.
- extentd keeps one instance of a store for its connections: its writer's, whose connection holds
  the path, and its followers' (`stores::followed`). Followers that find no writer open the store
  as a writer would; a writer opening it then takes that instance over
  (`ExtentStore::take_over`), as it does a writer's that went, if nothing is open in it (its last
  commit all it wrote): its followers go on following. A writer that went with writes
  uncommitted retires its instance: its followers are cut off (ESTALE), and no connection opens
  the store until they are gone (a writer's open fails with EBUSY meanwhile). A follower's guest
  unmounting is its going: it resets its device, which closes the connection, and mounting opens
  a new one, as of the writer's latest commit (doc-4's Reset).
- The daemon's work on a store followers have open: synced, grown and forked through their
  instance; not deleted, placed or made public (EBUSY).
- OPEN takes a read-only flag: a connection opening a store another connection holds gets a view
  of it, rather than EBUSY; with no writer connected, a view of the store as of its last commit
  (a writer may open it meanwhile).
- On a read-only connection, READ, MAP (read-only mappings only), ROOT and INFO are served from
  the view; WRITE, COMMIT, FREE, FORK and RESIZE fail with EROFS. ADVANCE moves the view, and
  PIN/UNPIN keep single extents (the graveyard).
- The writer's commits notify its stores' read-only connections (`COMMITTED`, from a thread of
  each that waits on the store's publishes, `Store::newer`, never on its commits); the device
  tells the guest with a configuration change (doc-4's `committed`).
- A read-only feature bit: the driver offers no write, and Fxfs mounts read-only.
- Firecracker's `extent` configuration takes `read_only: true` (a commit in the fxdax-v1.16.1
  series, the branch vendor/firecracker tracks; branch fxdax, on Firecracker's main, is
  superseded by it and has none of the changes since TASK-37).

### The follower in Fxfs (crates/fxfs, guest/rust/fxfs-linux)

Built (TASK-42.4): `crates/fxfs/src/follower.rs`, `FxFilesystem::{follow, hold}`, with
`Device::{advance, pin, unpin}` (storage_device) and `Journal::read_on`.

- Mount: Fxfs's read-only mount (`FxFilesystemBuilder::read_only`), which replays the journal in
  memory and writes nothing, on the view. When replay applied every transaction it read, the
  journal keeps its reader at the end of the last whole one.
- Follow (`FxFilesystem::follow`): advance the device (which returns what keeps the previous
  commit's frees, held until the batch is applied and its keeps pinned), read the journal on from
  that end, and apply each whole transaction, in order, store by store: the root parent store's
  mutations, the root store's, then the others' (opening stores the writer made), as replay
  applies them (`ApplyMode::Replay`: the same mutations of the trees, without the writer's
  reservations and object ID bookkeeping, which a follower has none of). The allocator's are left:
  a follower allocates nothing, so its free-space figures stay as mounted. A flush's last
  transaction (EndFlush) swaps the store's sealed layers for those the StoreInfo it wrote lists,
  as the writer's commit of it does; the mutable layer, which holds what came after BeginFlush,
  stays. The journal moving on to new extents is followed as replay follows it, from the
  root parent store's mutations of each committed transaction. A discard of transactions a
  follower applied, or a mutation that doesn't validate, fails it: it remounts.
- Each batch yields a change set (`Changes`): objects created or whose attributes changed,
  directories whose entries changed, data ranges changed (merged), objects deleted (unlinked from
  their last directory, or tombstoned), volumes deleted.
- The follower's graveyard (`FxFilesystem::hold`): a reader holds the objects it has open. When a
  transaction deletes one (its graveyard entry, or its tombstone, in one transaction or later),
  the follower first keeps its records as they are, and pins the extents they refer to; it skips
  the object's mutations from then on, and puts its records back after a flush swaps in layers
  without them. The open file reads as it was before its unlink. After the last hold goes, the
  next batch applies what it skipped, and unpins the extents.
- Fxfs's cached per-object state is the tree's object cache, which applying mutations
  invalidates as the writer's commits do; what a reader caches of an object (an open handle's
  size, the kernel's inode) is the change set's to invalidate.
- Readers are kept out of a follower's filesystem while it applies a batch, by its caller, until
  the concurrency protocol below (TASK-42.5).

### The follower in the guest (guest/linux, guest/rust/fxfs-linux)

Built (TASK-42.6):

- The driver offers `VIRTIO_EXTENT_F_READ_ONLY`, refuses what would write on such a device
  (-EROFS), sends ADVANCE, LET_GO (naming the epoch an ADVANCE returned: the device serves
  requests in any order, and a LET_GO of an earlier epoch lets go of no more), PIN and UNPIN, and
  reads `committed`. Its DAX window maps sealed extents read-only for a follower.
- fs/fxfs mounts read-only on `-o ro` or a follower's device (SB_RDONLY, which a remount keeps).
  On a follower's device, the device's configuration change schedules a batch (`fxfs_follow`):
  `follower::Protocol` over the kernel's primitives (follow.rs; C shims in super.c).
- Every VFS entry point that reads the trees holds the fill barrier shared until what it read is
  in the kernel's caches, or copied out (`fxfs_vfs_fill_begin`): lookup, getattr, readdir's fill
  of its buffer (dir_emit copies out after), readlink, extended attributes and ACLs, llseek's and
  fiemap's extents (a window at a time), statfs, file handles, open's hold, DAX faults and page
  cache faults (before filemap_fault's invalidate_lock), splice, fadvise and read(2). The Rust's
  tree reads check, in a debug kernel, that it is held (`Fxfs::reading`). A follower keeps no
  handles to objects: a batch would leave their sizes stale.
- read(2) holds it a chunk (1 MiB) at a time, copying with page faults disabled
  (`fxfs_vfs_follower_read`): a copy to a page of a mapping of a follower's file, which the fault
  would fill under the barrier, while a batch waits for it, would deadlock. One stopped short lets
  the barrier go, faults the reader's pages in, and goes on, as gfs2's reads do.
- A batch holds it exclusively (`fxfs_vfs_follow_barrier_take`, its wait counted:
  `FXFS_IOC_FOLLOW_STATS`): it drops what the kernel caches of what it changes
  (`fxfs_vfs_drop_file`, `fxfs_vfs_drop_entries`): a file's changed data drops its DAX window's
  ranges there under its invalidate_lock (with no inode read in, the file's ranges directly), or
  its page cache's pages; a directory's generation goes up, against which its dentries revalidate
  (`d_revalidate`, the generation read before each lookup in `d_time`); then it applies, reading
  the attributes it left each changed object with, which the inodes take (`fxfs_vfs_settle_file`:
  a file grown drops its ranges past its old end, which were as large as it was; a file shrunk,
  its mappings past its new end). It takes no inode's i_rwsem: the lock order is an inode's
  i_rwsem (the VFS's, a read(2)'s), then the barrier (after mmap_lock, in a fault), then an
  inode's invalidate_lock.
- A regular file's open holds it (`fxfs_hold`), its release lets go: one the writer deletes reads
  as it was until the last close.
- An extent written after the view's commit reads as ENOENT (Fxfs's journal replay reads on to the
  first extent its commit doesn't hold); one the view let go of, as EIO, logged (`NotHeld`).
- A filesystem mounted read-only, `-o ro` on a writer's device or a follower's, commits nothing,
  not even as it unmounts: its device knows it writes nothing (`ExtentDevice::writing_nothing`,
  TASK-42.6.1).

### Concurrency (the heart of it)

What must hold: a writer transaction is never seen half-applied (a rename visible in neither
directory, or both), and a reader that found a commit never finds, after, what a cache kept of an
earlier one (TASK-42.10). Each reader sees prefixes that only move forward, which is
linearizable, as a local disk is. A syscall needn't be atomic as a whole: a lookup that reads a
directory entry at one prefix and the object at a later one may find it deleted (ENOENT), as it
may racing a local unlink; a read(2) of several chunks may find a later commit in a later one.

**A fill barrier**, an rwsem per follower: readers hold it shared from before they read the trees
until what they read is in the kernel's caches, or copied out; a batch holds it exclusively from
before it drops what the kernel caches of what it changes until it has applied the change and the
kernel's inodes have taken its attributes. An rwsem's writer waiting goes before readers coming:
readers can't starve a batch, and a reader waits for at most one.

- No tree read overlaps an apply, so none sees half a transaction.
- What a reader cached before a batch, the batch dropped; what it cached after, it read whole: no
  cache is stale after a batch a reader found. Loads through DAX mappings take no barrier: a batch
  drops the mappings of what it changes before it applies.
- No reader copies from an extent a batch freed: it is done before the batch begins, and the
  follower lets go of the commit it was as of, whose frees its device keeps until then, after.

What it costs: readers wait while a batch drops and applies (milliseconds), and a batch waits for
the readers holding the barrier: a long read(2) for one chunk at most, a cold lookup for its I/O,
a DAX fault for its window's map. A writer's mounts take none of it: a follower's inodes have
operation tables of their own, and its other paths are behind the followers' static key. Measured (`FXFS_IOC_FOLLOW_STATS`; the torture test, release
kernel, GCE, 2026-10-19): without DAX, batches waited 160-210 ms in all over some 770 batches, 8
ms at most, and readers read as many as before the barrier (some 200k reads checked); with DAX,
13 s over some 590, 22 ms on average and 41 ms at most, as faults remap what a batch dropped
holding the barrier, and readers read 2 to 8 times fewer (7.5k-33k against 57k-67k).

What it replaced (TASK-42.10, 2026-10-19), and why: a seqcount around applies, each tree read
validated against it, retried, and after a few tries made under the follower's mutex; invalidation
per inode after the apply, under the inode's locks; and an SRCU grace period before the follower
let go of its commit. No reader stalled on a batch, but a batch invalidated after it applied:
between the two, a reader's fresh read found the new tree while its cached pages and mappings of
other files were still the old one's, so it saw a state no commit had (the torture test's shaped
file, TASK-42.10). Invalidating first, a fault that read old extents before the apply mapped them
after it. Closing both windows takes keeping fills out from before the drop to the end of the
apply: the barrier, which then makes the seqcount, the fallback and the grace period redundant.

The protocol (where a batch takes the barrier, drops, applies, settles, lets go; where readers
hold it) is Rust, generic over a small trait of the kernel's operations: take the barrier shared
or exclusively, drop what the kernel caches of a change, settle its inodes. In the guest, thin C
shims implement it with the kernel's own rwsem and locks, proven primitives that the torture test
(testing 6) runs under lockdep and KCSAN; in tests, Shuttle's primitives implement it, so the
testing below runs the real protocol code, not a model of it. We write no lock-free code of our
own with relaxed memory orderings: that is the one class of bug Shuttle can't find (testing 2).

## Testing (the plan's backbone)

Each layer below must exist, and find bugs, before the code it tests is trusted. Every technique
gets a mutation check, as doc-7's simulation has (TASK-28.1: each protocol rule has a test that
turns it off and expects a seed to find the violation): break the rule it guards (skip an
invalidation, release a pin early, apply half a transaction, skip the fill barrier) and the
test must fail. A test that never fails proves nothing.

Mutation testing covers the real code too, two ways:

- **The protocol's rules, switched off by hand.** Each rule in the Rust code (as in the TLA+ spec,
  and crates/extent_remote's `Rules`) can be turned off in tests, and each has a test that turns it
  off and expects the simulation, Shuttle or the history checker to fail.
- **Automatic mutants, with cargo-mutants** (sourcefrog/cargo-mutants, MIT): it rewrites the Rust
  code a mutation at a time (an operator flipped, a condition negated, a function body replaced by
  a default, a call removed) and runs the tests against each; a mutant no test kills is code the
  tests don't check. It runs over the follower's crates (store views, the follower, the protocol),
  its surviving mutants reviewed: each gets a test that kills it, or a reason it is equivalent.
  `scripts/mutants-followers.sh` runs it, each part against the tests that check it (the store's
  views against extent_store's tests and then its Shuttle tests; the Fxfs follower against the
  followers' simulation; the protocol under Shuttle; the history checker with the simulation's).
  Of what its first run left (TASK-42), tests kill all but three, which are equivalent: kept frees'
  release ranging from the commit released rather than after it (no free of a released commit is
  kept after its release), `Views::publish`'s `<` as `<=` (each commit's epoch is past every live
  view's), and a view's Debug (logs only).
  The kernel's C has no such tool: its rules are switched off by debug options in the torture
  test (6).

### 0. The protocol in TLA+, before any code

The design itself is model-checked first (TASK-42.8): `specs/follower/Follower.tla`, a TLA+ spec
of the writer (transactions of ordered key mutations over a tiny filesystem: a token renamed
between two directories, so that a half-applied rename shows as the token in neither; files
rewritten into new extents, the old freed; files deleted), the store (kept frees, releases, pins,
cut-off), the follower (batches waiting for the fill barrier, then, holding it, dropping each
changed file's mappings and applying mutation by mutation; advancing its view; the graveyard) and
readers (operations holding the barrier shared: pair reads, DAX faults, read(2)s, opens; loads
through their mappings, which take none; close), checked by TLC (tlaplus/tlaplus, MIT)
exhaustively. `make check-tla`
(scripts/check-tla.sh) runs every model in a few minutes; TLC's jar, pinned and checked, is
fetched once into ~/.cache.

- Safety, as invariants (models Follower: one reader, three commits, three operations, 5.0M
  states; FollowerReaders: two readers, symmetric, 2.0M; FollowerLoads: one reader loading
  through its mappings across a batch, 0.9M): reads see whole transactions, a committed prefix
  (NoTornReads); a reader's observations, across its operations and loads, never go back
  (NoGoingBack, TASK-42.10); no mapping, read(2) or graveyard copy uses an extent released
  (MappingsLive, NoReleasedReads, GravesLive); outside a batch, no mapping maps what a file held
  before (NoStaleMappings); a file deleted while open reads to its end (OpenFilesReadable); every
  pin is a graveyard's (NoLeakedPins).
- Liveness, under weak fairness of the follower, store and started reader operations
  (FollowerLive): the writer is never blocked by the follower (WriterNeverWaits, over ENABLED);
  the follower catches up; what the writer freed goes once the view is past it (FreesGo);
  readers' operations end; and even a follower that hangs is cut off
  rather than keeping the writer's frees without bound (FollowerStalled, LagCutOff).
- Mutation checks (specs/follower/mutations): each of the five rules removed (barrier,
  invalidate, keepFrees, graveyard, cutOff) gives a counterexample to the exact property it
  guards, within seconds; the barrier's, to each of three (NoGoingBack, NoTornReads,
  NoReleasedReads). Until TASK-42.10, the spec had the seqcount, its fallback and SRCU, and their
  rules (validate, fallback, grace, graceFirst), which the barrier replaced; with them, it had no
  loads through mappings, nor NoGoingBack, which is how it missed TASK-42.10's window.

What the spec leaves out, as the protocol doesn't depend on it: Fxfs's flushes and journal moves
(mutations like any other to the follower), files' many keys (one key each), the page cache path
(DAX's, without mappings), compression and object storage (below the store's view).

The spec and the code then meet: the simulation's histories (3) are checked against the spec by
TLC trace validation, so the spec stays the implementation's, not a separate story.

### 1. Loud failures (first, as it multiplies everything else)

- extentd checks each READ and MAP of a read-only connection against its view and pins: anything
  else is EIO and an error logged, never a silent stale read.
- The follower asserts it never advances past an epoch an operation in flight still uses.
- Code shape: reads return values; only validated values reach caches.
- Guest builds: WARN_ON on each invariant: a write request on a follower's device (the driver
  refuses it, -EROFS), a write fault through a follower's DAX window, a follower's epochs going
  back or settling past what it applied, a batch's invalidation off the follower's mounts; lockdep
  asserts the fill barrier held where the trees are read and batches change the kernel's caches.
  extentd's checks always on.

### 2. Shuttle: controlled schedules of the real code

[Shuttle](https://github.com/awslabs/shuttle) (Apache-2.0) runs a test with its own threads,
futures, locks and atomics in place of std's, and chooses every interleaving: randomly, with PCT
(which finds a bug needing d ordering constraints with a known probability per run), or
exhaustively by depth-first search for small cases (`check_dfs`). A failing run replays exactly
from its seed or schedule string. It is the engine of both the protocol's tests and the whole
follower's simulation (3).

The protocol's own tests run the real Rust protocol over the trait implemented with Shuttle's
primitives, in scenarios built to hit its windows:

- a batch applied against a reader's tree read (the barrier keeps them apart);
- a batch dropping a file's mappings against a fault of it;
- the view advancing against a read(2) still copying from an extent it frees;
- a follower under a busy writer: a reader still gets the barrier (a batch waiting for it keeps
  new readers out, then lets go).

`check_dfs` on the smallest (one batch, one reader, one fault) is exhaustive; `check_pct` and
`check_random` over many runs on the larger.

#### Why Shuttle, not loom

Both were examined (clones in ~/src/loom and ~/src/shuttle, 2026-10-17):

| | loom (tokio-rs, MIT) | Shuttle (awslabs, Apache-2.0) |
|---|---|---|
| Search | exhaustive, bounded (DPOR) | randomized (PCT, random), or exhaustive by DFS for small cases; replay by seed |
| Scale | at most 5 threads (`MAX_THREADS`); small scenarios | large: writer, follower, many readers |
| Async | `block_on` only; no executor for spawned futures | its own executor (`future::spawn`, `spawn_local`, `block_on`) |
| Weak memory | models C11 orderings | all atomics as SeqCst (documented) |
| Data races on UnsafeCell | detected | not detected |
| Wrappers | std | std, parking_lot (fuchsia-sync's, on the host), tokio, rand |
| Activity | 2 commits in the past year | 137 in the past year (last 2026-10-15) |

Fxfs is async, and the bugs feared take a writer, a follower, a reader and a fault together:
Shuttle fits, loom doesn't. loom's one advantage, weak memory orderings, matters only for
lock-free code of our own with relaxed orderings, which the protocol doesn't have: its barrier is
the kernel's rwsem. Should we ever write such code, loom tests it, alone.

### 3. Deterministic simulation of the whole follower, on Shuttle

Built (TASK-42.1, TASK-42.4): `crates/follower_sim`. The writer makes random file operations
(creates, writes, renames, unlinks purging in their transaction or after, syncs, compactions);
the follower follows; readers read files and holders hold files open through it. After each
batch, the follower's root directory, layers and root stores' records are compared with the
checker's own mount of the same commit, and its change set with what changed in its tree; the
history checker checks every read, and holders' reads of files deleted under them. Mutation tests
turn each of the follower's rules off (`fxfs::follower::Rules`), and simulate its bugs (reading
past its view, skipping changes, splitting commits), each caught.

The real Fxfs, writer and follower, on one store, with many reader tasks, in one process, under
Shuttle's scheduler, as FoundationDB and TigerBeetle test, and as crates/extent_remote/src/sim
does for doc-7. Shuttle is the deterministic executor: our fuchsia-async (tasks, timers) and
fuchsia-sync (locks, condition variables) get a `shuttle` feature backed by Shuttle's futures and
primitives, so Fxfs's code runs unchanged under it.

- Seeded workloads: creates, writes, renames, unlinks of open files, truncates, flushes, journal
  moves, mixed with follower batches and reader operations.
- Every await and lock is a scheduling point Shuttle chooses at, so the windows between a batch's
  mutations and inside tree reads come up constantly; notifications are delayed and reordered.
- Any failure replays from its seed; `SIM_SOAK` runs many more seeds.
- Runs on macOS in seconds, in `cargo nextest run`.
- To check first: what in Fxfs runs on threads of its own, or blocks outside fuchsia-sync (a
  std lock, a channel); each needs the `shuttle` feature too, or Shuttle can't schedule it.

### 4. History checking (linearizability)

Record each reader operation with the commits applied when it started and when it ended. A
checker requires each result to equal the writer's state at some commit in that range, and each
reader's commits never to go back. The oracle is a view of commit c, which answers what was true
at c exactly. This catches any read of a state the writer never committed, half-applied
transactions included. Runs over the simulation's histories (3) and the guest torture's (6).

### 5. Comparing each batch

After each batch, with the follower quiet, walk its trees against the writer's as of that commit
(a view of it): every object, attribute, directory entry and extent. This catches apply bugs
(flushes, journal moves, deletions) apart from concurrency bugs.

### 6. Guest torture, as Linux's rcutorture

Two or more VMs, a writer and followers, on a debug guest kernel (KCSAN for data races, lockdep
for lock order, DEBUG_ATOMIC_SLEEP), DAX on and off, with workloads whose invariants check
themselves:

- a token renamed back and forth between directories (seen in neither or both: a bug);
- files with a generation and checksum in each block (torn or stale: a bug);
- mmapped readers verifying pages under DAX while the writer truncates, extends, rewrites;
- files deleted while followers have them open, read to the end.

Delay injection points (debug builds) widen the windows: between a batch's apply and its
settling, between iomap_begin and the PTE insert in a fault (as seal_race widened the one that
found TASK-39.6's DAX bug). Mutants switch each rule off and must be caught: the batches' drops
(stale reads, a wrong history), the readers' barrier (a fault maps an extent a batch let go of,
which extentd refuses, and the follower's VMM aborts on the fault), the read(2)s' copies without
page faults (a reader reading into a private mapping of a follower's file hangs). Hours on GCE, several followers; the follower's lag and pinned bytes
recorded.

Built (TASK-42.7): `fxfs-guest-follow-torture(-dax)`, a writer and two followers, and its
mutants: the batches' drops off (with DAX and without, and at the fault window), the barrier off,
read(2)'s copies faulting. How a mutant's followers fail depends on the schedule (a stale page, a
VMM's abort on an extent let go of, a reader stopping on what it can't find); one is caught when
no follower passes and the history is wrong. The writer writes back each data write before its
next operation, so that its operations reach the journal in the order it makes them, as the
history checker's model assumes: written only to the page cache, a write reaches the journal at
writeback, after the operations since (a size changed after it commits before it; a file deleted
before its writeback never held it), which an hour's run found without DAX (TASK-42.12). On a
debug kernel, KCSAN's reports fail the test when they name the followers' code (fs/fxfs,
virtio_extent, their Rust), but for a race whose change is PF_MEMALLOC_NOFS alone (fs/fxfs's
NOFS scopes writing their own task's flags); the kernel's own races, each seen looked into, it
leaves to the kernel.

### 7. The writer never waits, measured

The writer's commit latency and pgbench tps with followers attached, against none: no
difference beyond noise.

Measured (docs/measurements): a writer alone runs as it did before the follower code, and before
the fill barrier, within noise (gce-fs-bench-ac5-*, TASK-42.7). With followers attached it does
not yet: pgbench 1.28x slower with two reading, 1.43x with two idle (TASK-42.11). Not the
followers' CPUs (the followed case's writer pinned alike, no followers started, runs as fast as
alone), nor their reads' disk I/O (reading through the extent cache made it worse): the store's
state lock, which the writer's writes, frees and commits take, waited on ~10 s in a run with
idle followers against 0.5 s alone, held meanwhile by the uploader's closes of the open segment
(`Store::close_now`, framing and syncing under the lock), which it makes back to back, seven
times as often, with followers (TASK-41.1).

## Open questions

- The cut-off limits (decision 7): lag in seconds, pinned bytes.
- How a follower's guest is told of commits: an event queue, or configuration changes.
- Whether Fxfs's per-object cached state (handles' sizes, overwrite ranges) can be refreshed in
  place, or handles must be dropped and reopened.
- What Fxfs runs on threads, for the simulation (3).
