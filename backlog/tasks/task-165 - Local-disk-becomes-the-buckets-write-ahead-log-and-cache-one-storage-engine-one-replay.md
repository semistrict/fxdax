---
id: TASK-165
title: >-
  Local disk becomes the bucket's write-ahead log and cache: one storage engine,
  one replay
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-04 07:34'
updated_date: '2026-11-07 13:16'
labels: []
dependencies:
  - TASK-164
priority: high
type: task
ordinal: 610000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decided by the user on 2026-11-04 (docs/reviews/2026-11-04-first-principles.md, section 2), to start after the read-only DAX work. extent_store's local lineage (records, layers, ledger, index, cleaning, rebuild) and extent_remote each hold every branch, crates/fxdax/src/remote.rs keeping them agreeing; 13+ bugs at the seam, three footer replays (160.15), two cleaners, fork bugs fixed three times. New design: local disk holds only unacknowledged closed segments and a part cache; forks, points, records, checkpoints and cleaning exist only in the bucket protocol; one replay; a local-filesystem Bucket for development. Then a serial commit log over parallel data (a SlateDB-style manifest) and a TLA+ spec of doc-7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A design doc, with what goes and the format change
- [ ] #2 One engine, one replay, the workspace and guest suites passing, the seam's tests kept as tests of the bucket protocol
- [x] #3 A TLA+ spec of the doc-7 protocol with mutations
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Design doc (doc-14): what goes, local disk afterwards, formats, the one replay, the file bucket, tests, steps.
2. Subtasks TASK-165.1 to 165.10, one per step of doc-14, with acceptance criteria and dependencies.
3. Decisions in OPEN_QUESTIONS.md.
4. Step 1 (TASK-165.1, the file:// bucket) on this branch, least risk, additive.
The rest per the subtasks, after TASK-164 and TASK-160.8 for steps 7 to 9.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: doc-14 (2026-11-04). Inputs: review section 2, doc-7, the code of extent_store, extent_remote, remote.rs, extents.rs's fork path. Seam bugs classified into four classes the design removes (doc-14, Why). Steps: TASK-165.1 to 165.10. Decisions: OPEN_QUESTIONS.md, "Local disk as the bucket's WAL and cache: how".

2026-11-05: TASK-165.3, 165.4, 165.5 and 165.6 done on main (GCE r691 and r690, CI 42137709782, macOS workspace 2118 of 2118 at 752515020). AC #3 met: specs/bucket's Bucket.tla (TASK-165.2, done) models doc-7 with the WAL; check-tla.sh 11 of 11 models hold and 32 of 32 mutations give their counterexamples (TASK-165.5's run after its rebase), 43 of 43 at 752515020's triage. AC #2 open: TASK-165.7 to 165.10 are To Do (the local lineage, cleaning and index still there).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Local disk as the bucket's WAL and cache: how (TASK-165, doc-14, 2026-11-04)**

The user chose the direction (review of 2026-11-04, decision 2); doc-14 decides the rest, where the task left it open:
- **WAL segments are stamped as they close**, by the writer's chain: each is the object byte for byte (doc-7, decision 1), and footers alone tell, after a takeover, which landed; the rest are restamped on the new chain if they follow the branch's end, by commits (changed by TASK-165.2, below). *Instead:* an unstamped WAL stamped at upload, which needs a record of every lease epoch the store took to tell its commits in the bucket from another host's: remote.rs's marker again. *Undoing it:* `wal::rebase` and the WAL's file format (TASK-165.5, 165.9).
- **A commit's local durability is a log beside the open segment** (a record of each commit's `Contents`, hashed, synced by the flush), folded into the footer when it closes. *Instead:* a segment closed per flush, 20 PUTs a second per VM; or keeping the index's deltas, a second encoding of the branch's state. *Undoing it:* TASK-165.9.
- **A WAL that doesn't follow the bucket refuses to open** (`Error::Diverged`, `ESTALE`) until `fxdaxctl take` drops it, as a stale store refuses now. *Instead:* drop it silently: the guest's restarted VM would see a history it never committed. *Undoing it:* one arm of the open path.
- **Capacity is branch state, set by commits** (footers and checkpoints): resizes are ordered with commits, forks inherit, points never change. *Instead:* a field of the record, updated by compare-and-swap, unordered with the commits it bounds. Format change (FORMAT 2).
- **One spending rule:** every footer free of the branch's own extents spends it (footers already free only extents a commit held). Remote recovery's "held before the segment" rule goes: it kept an ID sealed and freed within one segment writable elsewhere, which nothing needs, each writer minting from its own epoch's range. *Undoing it:* the rule in `apply_contents` (TASK-165.3).
- **Formats:** objects of any format but the reader's are refused, older ones too; today's local stores aren't opened or converted (AGENTS.md: nothing uses fxdax yet).
- **extent_store depends on extent_remote, and the uploader moves into it**, so that the host's side of the seam is tested against a memory bucket in one crate; `fxdax::remote` keeps the host's configuration, budgets, runtime and errno mapping. *Instead:* keep the uploader in fxdax, with its tests a crate away from the store they drive.
- **The part cache is kept across restarts**, its index rebuilt from its files: without a full local copy, an extentd restart would otherwise refetch every base.
- **The collector is the only cleaner;** the bound of three times the capacity on local files (TASK-79.89) goes with local dead space. The host's disk is bounded by the lag bound, the open segment's room and the cache's capacity.
- **Views (followers, frozen) keep working** on the new engine; a view reading an extent the collector reclaimed a grace period after its free is cut off. *Instead:* hold the collector off for views, which it can't see on another host.
- **The file bucket** is object_store's `LocalFileSystem` with conditional updates, and overwrites and deletes, under a bucket-wide `flock` (each holding opening the lock file anew, so that it excludes holdings in its own process too), e-tags being `LocalFileSystem`'s (inode, modification time in µs, size). *Instead:* a `Bucket` of our own beside `Store`, which `Store::from_url`, its stats and its throttle wouldn't cover. The e-tag could repeat only if a replaced file's inode were reused with the same size within its microsecond, the clock not going back.
- **The TLA+ spec comes before the protocol's code** (TASK-165.2, then 165.5), modelling the WAL's rebase; it is TASK-44.5's AC #1 too.
- **Steps 7 and 8 land in one batch**, so that main never has forks on one host reading their base back from the bucket.
- **A fork has no claim file:** its child's ID is checked with one GET; a fork whose answer a crash lost stands if its segment landed (the parent's next writer records it) and never was if the disk went with it; a fork fenced before its acknowledgement deletes its child by its owner (`branch::delete_fork`).
- **Images keep their branches in a file bucket named in their description**, their clones sharing it, their collector in-process (TASK-165.6).

Lines removed and added (TASK-165.10, 2026-11-07), against doc-14's estimates. Code is a file's lines before its #[cfg(test)] module, as doc-14 counted at 9f81f6018; main is 213504c4b. Other work changed some of these files meanwhile (TASK-160's refactors, TASK-149's FUSE, TASK-154's tenants); the commit counts below are the TASK-165.1-165.9 commits' alone.

By module, code at 9f81f6018 -> main (doc-14's estimate):
- extent_store lineage.rs 574 -> gone (goes); branch_records.rs 428 -> gone (goes); layers.rs 430 -> gone (goes); ledger.rs 447 -> gone (goes); index.rs 564 -> gone (goes).
- extent_store host.rs 411 -> 237 (~100 stay); uploads.rs 141 -> 55 (goes; Closed, a segment for the uploader, stays); kept.rs 208 -> 219 (~150 go; stayed: frees kept until acknowledged); layout.rs 546 -> 381 (~300 go; 165 went); segments.rs 1,367 -> 979 (~500 go; 388 went); state.rs 988 -> 1,109 (~400 go; garbage, the 3x bound and file keys went, one replay, Life and Standing came); store.rs 2,304 -> 2,316 (~1,100 go; fork*, rebuild, clean*, adopt, spend, snapshot, upload, release and lineage plumbing went, opening by recovery, rebase, leaving into the cache, framings as one queue, timings came).
- extent_store new: wal.rs 469, upload.rs 1,112 (moved from remote.rs), timings.rs 160 (doc-14: ~1.3k come, with the file bucket and the cache).
- extent_store src: code 10,757 -> 9,580 (-1,177; doc-14: -4.1k, 5.4k going and 1.3k coming); with unit tests 13,315 -> 11,259; tests/ 7,779 -> 5,930 (tests/lineage.rs 280 and tests/shuttle/lineage.rs 489 gone, tests/store.rs 3,744 -> 2,627).
- fxdax remote.rs code 1,687 -> 178 (~1,300 go, ~400 move; 1,509 went), with its tests 4,215 -> 2,597; host_store.rs 191 -> 443; extents.rs: fork_durable's claims, disclaims and unforks went, and copy_bases (doc-14 said it stays).
- extent_remote: local.rs 0 -> 212 (file bucket), wal.rs 0 -> 147 (Wal::rebase), cache.rs 316 -> 905 (kept across restarts, adopt), recovery.rs 500 -> 670.

TASK-165.1-165.9 commits (git log --grep 'TASK-165\.[1-9]' 9f81f6018..213504c4b, --numstat, code and tests): extent_store +6,466 -11,096 (src +5,181 -7,797; tests/ +1,285 -3,299); fxdax +3,803 -4,839 (remote.rs +953 -2,913, host_store.rs +690 -532, extents.rs +576 -598, storage/kept.rs +383 -36); extent_remote +4,073 -809 (cache.rs +1,589 -333, wal.rs +447, local.rs +259, recovery.rs +272 -107); extent_frame +152 -125. In all about 2.4k lines fewer, against the review's 7-8k: one recovery from the bucket and the WAL, crash-tested at each write, and the part cache's persistence are larger than doc-14 guessed, and kept frees stayed. The review's section 2 is marked done (docs/reviews/2026-11-04-first-principles.md); doc-14's What went has the table.
<!-- SECTION:NOTES:END -->
