---
id: doc-8
title: Live read-only followers of a mounted Fxfs
type: specification
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:29'
---
# Live read-only followers of a mounted Fxfs

Status: planned, 2026-10-17; nothing built yet. Tasks: TASK-42, in order: the protocol in TLA+
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
   followers cost the writer's space. Limits to be chosen with measurements.
8. **No global lock** stalls a follower's readers while it applies: see Concurrency.

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

- `ExtentStore::view()`: the store as of its latest commit: the extent table, sealed lengths and
  root at that commit, and its epoch. `View::advance()` moves it to the latest commit.
- A free of an extent that a live view as of an earlier epoch holds is kept: not deleted, its
  segment pieces still counted live (so compaction leaves them), until no view older than the free
  is left, nor the uploader needs it. The kept bytes count as used.
- Pins of single extents beyond the view's epoch (the follower's graveyard, below).
- Views live as long as their holder; a view cut off (decision 7) fails its reads.

### extentd and the device protocol (doc-4)

- OPEN takes a read-only flag: a connection opening a store another connection holds gets a view
  of it, rather than EBUSY; with no writer connected, a view of the store as of its last commit
  (a writer may open it meanwhile).
- On a read-only connection, READ, MAP (read-only mappings only), ROOT and INFO are served from
  the view; WRITE, COMMIT, FREE, FORK and RESIZE fail with EROFS. ADVANCE moves the view, and
  PIN/UNPIN keep single extents (the graveyard).
- The writer's commits notify its stores' read-only connections; the device tells the guest
  (a used buffer on an event queue, or a configuration change).
- A read-only feature bit: the driver offers no write, and Fxfs mounts read-only.
- Firecracker's `extent` configuration takes `read_only: true` (a commit in the fxdax-v1.16.1
  series, and on branch fxdax).

### The follower in Fxfs (crates/fxfs, guest/rust/fxfs-linux)

- Mount: Fxfs's read-only mount (`FxFilesystemBuilder::read_only`), which replays the journal in
  memory and writes nothing, on the view.
- Follow: on each notification, advance the view, read the journal on from where replay or the
  last batch stopped, and apply each whole transaction to the live trees, as the writer's own
  commits apply their mutations to live trees (not through mount-only replay paths). It handles
  flushes (EndFlush: new layer files swapped in), the journal moving to new extents (a new
  super-block), deletions (graveyard, tombstones), and Fxfs's cached per-object state.
- Each batch yields a change set: objects whose attributes changed, directories whose entries
  changed, data ranges changed, objects deleted.
- The follower's graveyard: before applying the deletion of an object a reader has open, the
  follower keeps a copy of its extent list and pins those extents; the open file reads through
  it until its last close, which unpins them.

### Concurrency (the heart of it)

What must hold: a writer transaction is never seen half-applied (a rename visible in neither
directory, or both). Each tree read sees whole transactions; then any sequence of reads sees
prefixes that only move forward, which is linearizable, as a local disk is. A syscall needn't be
atomic as a whole: a lookup that reads a directory entry at one prefix and the object at a later
one may find it deleted (ENOENT), as it may racing a local unlink.

Why not one rwsem around each VFS operation: a long read(2), or a cold lookup doing I/O, holds it
for milliseconds or seconds; the follower waits behind it, and new readers queue behind the
follower (rwsems favour a waiting writer). Readers stall in bursts, the follower's lag and pinned
space grow.

Instead, three mechanisms, none of which stalls all readers:

1. **A seqcount around applies.** The follower bumps a sequence number before and after applying
   a batch. Each tree read (a lookup's entry, a getattr, iomap_begin's extents) checks it and
   retries if it moved; after a few retries it takes a lock instead (read_seqbegin_or_lock), so a
   busy writer can't starve it. It is the kernel's seqlock_t, not one of our own. Fxfs's trees already allow readers
   alongside a mutator. Nothing goes into a kernel cache before its read validates.
2. **Invalidation per inode, under the kernel's own locks.** After a batch, the follower walks the
   change set: for DAX, it takes each changed inode's invalidate_lock exclusively and breaks the
   layout of the changed ranges (user mappings and DAX entries); a fault, holding invalidate_lock
   shared and its DAX entry's lock, that read extents before the apply and maps them after it is
   torn down by this step. Sizes and attributes are refreshed under the inode's lock; directories'
   generation numbers go up, and d_revalidate drops their stale dentries (as FUSE and NFS do).
3. **A grace period before pins go.** A read(2) copying from an old extent finishes before the
   follower's view moves past it: the kernel's SRCU, over operations holding iomaps. Advancing
   waits for it; this delays releasing space, never the writer nor the readers.

The protocol (the order apply, invalidate, wait for the grace period, advance; where reads begin,
validate and fall back) is Rust, generic over a small trait of the kernel's operations: begin and
validate a read, take the fallback lock, break a layout, bump a generation, refresh an inode,
enter and leave the grace period, wait for it. In the guest, thin C shims implement it with the
kernel's own seqlock_t, SRCU and locks, proven primitives that the torture test (testing 6) runs
under KCSAN; in tests, Shuttle's primitives implement it, so the testing below runs the real
protocol code, not a model of it. We write no lock-free code of our own with relaxed memory
orderings: that is the one class of bug Shuttle can't find (testing 2).

## Testing (the plan's backbone)

Each layer below must exist, and find bugs, before the code it tests is trusted. Every technique
gets a mutation check, as doc-7's simulation has (TASK-28.1: each protocol rule has a test that
turns it off and expects a seed to find the violation): break the rule it guards (skip an
invalidation, release a pin early, apply half a transaction, drop the seqcount check) and the
test must fail. A test that never fails proves nothing.

### 0. The protocol in TLA+, before any code

The design itself is model-checked first (TASK-42.8): a TLA+ spec of the writer (transactions,
commits, frees), store views (epochs, kept frees, pins, cut-off), the follower (advance, batch
apply, seqcount, per-inode invalidation, grace period, graveyard) and readers (tree reads with
retry and fallback, faults mapping extents, read(2) in flight, files deleted while open), checked
by TLC (tlaplus/tlaplus, MIT) exhaustively over small bounds, and by Apalache (Apache-2.0) where
TLC's bounds are too small.

- Safety: every read sees a committed prefix of the writer's history; a reader's prefixes never
  go back; no read touches an extent released; no mapping of an old extent outlives the
  follower's advance; a file deleted while open reads to its end.
- Liveness, under fairness: the writer is never blocked by a follower; a follower advances;
  readers are not starved; pins go after close or advance; a lagging follower is cut off rather
  than pinning without bound.
- Mutation checks: each rule removed (validation after reads, invalidation after apply, the grace
  period, keeping frees for older views, graveyard pins) gives a counterexample.

The spec and the code then meet: the simulation's histories (3) are checked against the spec by
TLC trace validation, so the spec stays the implementation's, not a separate story.

### 1. Loud failures (first, as it multiplies everything else)

- extentd checks each READ and MAP of a read-only connection against its view and pins: anything
  else is EIO and an error logged, never a silent stale read.
- The follower asserts it never advances past an epoch an operation in flight still uses.
- Code shape: reads return values; only validated values reach caches.
- Guest debug builds: WARN_ON on each invariant; extentd's checks always on.

### 2. Shuttle: controlled schedules of the real code

[Shuttle](https://github.com/awslabs/shuttle) (Apache-2.0) runs a test with its own threads,
futures, locks and atomics in place of std's, and chooses every interleaving: randomly, with PCT
(which finds a bug needing d ordering constraints with a known probability per run), or
exhaustively by depth-first search for small cases (`check_dfs`). A failing run replays exactly
from its seed or schedule string. It is the engine of both the protocol's tests and the whole
follower's simulation (3).

The protocol's own tests run the real Rust protocol over the trait implemented with Shuttle's
primitives, in scenarios built to hit its windows:

- a batch applied against a reader's tree read (the seqcount retries, and falls back);
- invalidation against a fault that read extents before the apply and maps them after it;
- the view advancing against a read(2) still copying from an extent it frees;
- a follower under a busy writer: a reader is never starved (the fallback bounds retries).

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
lock-free code of our own with relaxed orderings, which the protocol doesn't have: its seqcount
and grace period are the kernel's. Should we ever write such code, loom tests it, alone.

### 3. Deterministic simulation of the whole follower, on Shuttle

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

Delay injection points (debug builds) widen the windows: between apply and invalidation in the
follower, between iomap_begin and the PTE insert in a fault (as seal_race widened the one that
found TASK-39.6's DAX bug). Hours on GCE, several followers; the follower's lag and pinned bytes
recorded.

### 7. The writer never waits, measured

The writer's commit latency and pgbench tps with followers attached, against none: no
difference beyond noise.

## Open questions

- The cut-off limits (decision 7): lag in seconds, pinned bytes.
- How a follower's guest is told of commits: an event queue, or configuration changes.
- Whether Fxfs's per-object cached state (handles' sizes, overwrite ranges) can be refreshed in
  place, or handles must be dropped and reopened.
- What Fxfs runs on threads, for the simulation (3).
