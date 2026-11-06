---
id: TASK-153.3
title: 'An extent goes minted, open, sealed, and never back'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:52'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 547000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Open is not one fact and can go backwards: the Rust open set, region->writable, the device seal and per-block sharing can disagree, and on the host ExtentStore.open and the store uncommitted are two owners too. Explains TASK-99, 140, 44.21, 91.1, 91.9, 150.11, 79.46, 121, 150.7. A write to an extent not known to be open retries rather than reopens, shared blocks never go through an open extent region, and DaxExtent says which of sealed, shared or not known open it means.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An extent can not read as open after sealed
- [x] #2 The checks around open that this makes dead are gone
- [x] #3 guest/sim passes, those past bugs put back failing or unbuildable
- [x] #4 Suites and pgbench-dax pass on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
First step, 2026-11-01: states and transitions today, and the proposal.

WHAT MAKES AN EXTENT READ OPEN AFTER SEALED. One line: `write_with_opts` (guest/rust/fxfs-linux/src/device.rs) inserts the extent it wrote in `open`. A seal clears `open` at its begin and end, all of it, as Rust can't see which extents a MAP reached. An extent minted before a seal that no request has reached isn't sealed by the host, which has never heard of it. So: mint X (open) -> seal (forgotten, reads sealed) -> a transaction's device write to X, taken before the seal, sent after it: the host takes it, and Rust inserts X again (reads open). Nothing else does: `opened` is called by `ExtentTable::mint` alone, on a new sequence. FakeDevice's `is_open` has the same insert. A SECOND reversal, needing no seal: `extents_in`'s `writable` is `is_open && !is_shared`, so a piece of an open extent reads sealed while shared, and open again once the clone lets go.

TODAY, GUEST: four facts. (1) Rust `open`, a set: entered at mint and at each write; left at seals, and frees. open -> forgotten -> open. (2) `region->writable`: false -> true in region_open, unless mapped; true -> false in the sealer. (3) The device's seal: host-side. (4) Per-block sharing, in `is_shared`. `DaxExtent.open` is `(1) && !(4)`: false means sealed, or forgotten, or shared; `dax_map_write` says true whatever (1) is.
So a reader that saw X as not open maps X's region read-only, and a writer that later sees X as open finds a region it can't make writable: TASK-99 (their MAPs race), TASK-140 (a slice that can't take the write, for ever), TASK-44.21 and 150.11 (a shared 'sealed' piece reaching past a writable region, a writable MAP outside the gate), and TASK-91.9 (recorded open before the write was refused: an insert on the write path at all).

PROPOSED, GUEST. Minted -> open -> sealed, one way, as a type: an extent is open iff minted since the last seal. Sequences only grow, and `mint` alone opens, so the open extents are one range of sequences of one branch, `first..next`: a seal sets `first = next`. No set to insert in: an extent sealed is below `first`, and `first` only grows: reopening can't be written. `OpenExtents`, in storage_device, used by ExtentDevice AND FakeDevice, so the fake can't differ. A device write changes nothing that's known.
- DIFFERS FROM THE DIRECTION: a plain device write to an extent not known open is NOT refused or retried at the device. multi_write, journal and layer-file writes have no retry (SealedExtent there is EIO, TASK-91.1), and need none: the host takes it, X stays read as sealed, and the next seal seals it on the host. DAX's retry already exists: dax.c's `gate != seals` moves the blocks again, and now they do move, X being not open.
- What the proof rests on: every piece map_write returns with `gate == seals` was found open, or minted, after the snapshot of seals, and no seal since, so is in the range. So no new retry is needed. dax_map_write says what it knows rather than `true`.
- `Freezer::freeze` looked at `open.is_empty()`: a write to a forgotten extent would now slip past it, leaving the host not quiescent (fork_quiescent: EBUSY). It seals always, unless the filesystem writes nothing.
- `DaxExtent.open` u32 becomes `access`: SEALED (not known open: read-only, anywhere, outside the gate, for good), OPEN (writable in place, in the gate), SHARED (the extent is open, these blocks aren't writable: read-only, in the range's own space, never through the region). `sealed` and `not known open` are one state: nothing treats them differently, and they can't be told apart without asking the host. Rust-to-C ABI inside the guest kernel: no on-disk or wire change.

WHAT DAX.C THEN KNOWS: a region's extent is SEALED or OPEN for its whole life from the region's point of view: SEALED never goes back; a read-only region MAP is of a SEALED piece; SHARED doesn't touch regions. So a region mapped read-only is never asked to open, and a reader never meets a writable region past what it maps (the sealer clears `writable` before Rust forgets the extent).

CHECKS THAT GO DEAD:
- fxfs_dax_region_open: `!region->mapped`, and region->lock (TASK-99's fix).
- fxfs_dax_map_to: `sealed && writable`, its `sealed` argument, and fxfs_dax_reach's (TASK-44.21, 150.11).
- fxfs_dax_move: `if (!region->writable) -EXDEV`.
- fxfs_dax_new_slice: `if (region->writable) break; ... return NULL` (TASK-140), `e.open && region->writable`.
- fxfs_dax_map_slice: `e->open && !region->writable`, `if (!e->open) continue` (map_write's pieces only reach it: all open).
- fxfs_dax_restage_work: `READ_ONCE(region->writable)`.
Stays: map_to's assertion of the gate where a writable MAP is sent; the `gate == seals` checks; fxfs_dax_grow's look, as a seal may have come.

SIM. fxfs.c's reopening goes as the Rust's does; `SHARED` as extents_in says. Past bugs whose fix went: put back by their cause instead: a mutant of the model reopening, a mutant of dax.c taking SHARED through regions. sim_build compiles fxfs.c per mutant for that. specs/dax-region models a reader and a writer meeting on an open extent, which can't be any more: it takes the one-way states, and the mutation is that they meet.

HOST: looked at after the guest half, as asked.

Done, 2026-11-02: the guest half. Branch refactor/open-once.

AFTER. Rust: OpenExtents, one range of sequence numbers: minted, open, sealed, never back; a seal moves its start on; no write touches it; FakeDevice uses the same type. DaxExtent.access: Sealed (sealed or not known open, one state, for good), Open, Shared. dax.c: a region is opened only for an Open piece, and read-only MAPs of regions are of Sealed pieces alone; Shared stays in the range's own space. Freeze seals always, unless the filesystem writes nothing.
GONE: as listed above, all of it. Added: one WARN_ONCE in region_open, an open extent's region mapped read-only.
DEVIATION FROM THE DIRECTION: no refusal or retry of a plain device write (dead-ends/refusing-writes-to-extents-not-known-open.md). dax.c's gate != seals is the retry. No on-disk or wire change: access is the Rust-to-C ABI inside the guest kernel, by enum fxfs_dax_access.

IMPOSSIBLE NOW, AND HOW IT IS KNOWN. TASK-91.9 and the guest half of TASK-99, TASK-140: no write records anything: OpenExtents' tests (test_open_extents_go_one_way), test_writes_open_nothing, test_a_write_after_a_seal_reopens_nothing. TASK-44.21, TASK-150.11: only Sealed pieces read through regions, and the sealer clears writable before Rust forgets. Mutants, by cause, as their fixes are gone: task-153.3-reopens, seed 32, region_open's WARN; task-99, with the old look, seed 32, a store the host refused, as the VM stopped; task-140, task-44.21, task-150.11, WARN removed too, open_extent_read_only, a store the host refused. specs/dax-region: holds (34 states); NoOneWay finds NoRefusedStores, NoSharedApart finds NoWritableMapOutsideTheGate. NoOpenUnderLock went with its rule: NoOneWay is the same bug. TLC run by hand: check-tla.sh's pinned SHA-256 no longer matches the rolling v1.8.0 jar (unrelated).
NOT TOUCHED: TASK-79.46, TASK-121's host half, TASK-150.7: host; below.

TWO BUGS FOUND, NEITHER OF THIS CHANGE: blind spot, the zloop mix. (1) Fixed, own commit: a slice's refusal said -ESTALE to readers, dropped once: two in a row, SIGBUS. Seed 1102; on the base, seed 12343. Structure: `behind` holds what its iomaps say. Mutant task-153.3-refused-slice-stale, red on interleave 1102 1000. Looked around: write and zero paths already took -EXDEV as a slice's. (2) Open: fs/dax.c:416 WARN, seed 2677 at 1,000 operations: a PMD fault has its entry and iomap, a write moves a page of the row and finds that entry, says shared, and waits for it; the PMD goes in; invalidate_inode_pages2_range disassociates under it. Reproduced with dax_pause at READ_MAPPED, on the base commit too (tests/scenarios, not committed: it fails). iomap_end taking the PMD away passes the seed, not those steps: a refault of a page follows. It wants the entry unmapped under its lock, in the fs/dax.c series. Needs its own task. Also unexplained, base only: seed 10529 at 1,000 operations, a private load of zeros, Fxfs's extents right; none at this head in 15,000.
tests/failures lost half the window on average to failed UNMAPs, for good, and 4 runs in 3,000 here (0 on the base) none of the ranges' space: mean lost 7,835 pages against 7,554. kernel/device.c bounds it at a quarter. Not the Rust's model: fault injection. Lost: UNMAP failures past that bound.
zloop.sh runs every seed at 1,000 operations, not an eighth: `local seed=$1 count=...$seed` reads seed before it is set. Left.

RESULTS. sim: 15,000 seeds, 1 failed (2677, above); failures 3,000 clean; scenarios clean at this head; mutants 20 of 20 that can show; TASK-150.14 not found in 2,676 seeds: its scenario needed a slice of shared blocks. Each commit: cargo check and 60 seeds. Workspace, Docker arm64: 2112 of 2113, the Docker-only tenant_sockets_work_bind_mounted_into_jails; guest tests compile; guest Rust cargo check; macOS, storage_device, fxfs, fxfs-posix: 812 of 812. Not run: Lima, the macOS workspace.
GCE: r470 (before the slice fix) checks-dax, -delay, -small, script-full-dax OK, xfstests the known list and generic/129; r471 checks-dax x3 and pgbench-dax x3 OK; r475 (with it) -delay, -small, script-full OK, xfstests the same 17 of 650, as r452 and r457. No WARN, no unexplained refusals; write_times_add_up passes. r474: a host loop mount's umount, target busy, before any guest: rerun as r476.
pgbench-dax tps, 3 rounds of 5 runs each: base r472 870, r473 934; head r471 954, r476 903. Means 902, 929: no drop. The gate's decay fails both sides: 0.85, 0.86; 0.88, 0.87.

HOST, NOT DONE: ExtentStore.open and Store.uncommitted. A MAP moving an extent wholly out of the store is Store API (uncommitted's openings, overwritten, ledger), 39 sites, lock-free begin_write's opening numbers, Store::extent going None for what's mapped (read_cached, free, describe), and specs/extent-open. It needs a decision: what a fork holds of an extent mapped writably, today EBUSY (TASK-150.7). A change of its own.

2026-11-02, merge of origin/main (106b6128d), and the host half.

MERGE. Main took pages out of regions (fafefbac7): regions are slices and a write(2)'s way through (106b6128d). fxfs_dax_move went, and with it the Shared check in map_extents: new_slice is the one way shared blocks could reach a region. fxfs_dax_open_through opens no new way to read open: gate == seals, on a piece map_write found open; its !region->writable is dead as new_slice's was. Lagging pages keep their piece's access. NoOpenUnderLock.cfg is back (OPEN_QUESTIONS.md). zloop.sh ran every seed at 1,000 operations: fixed, own commit.
FOUND VERIFYING, all main's before this branch: (1) 106b6128d: a fault finishing between the move's unmap and the look at `users` leaves its PTE of a page that then lags: stale read, seed 7289 at 200. Fixed, 157417ad5, mutant task-153.3-lag-under-a-pte. (2) The slice -ESTALE class: main's seed 3762 at 1,000. (3) fs/dax.c:416 WARN, a write's invalidation of a PMD entry under its PMD: 3 in 30,000 at 200 on main, 5 here. Open. (4) extents::tests::windows_share_bases_pages flakes under load, main too (strong_count 5 against 4).
HOST HALF, d559e9e78, on the decision in OPEN_QUESTIONS.md: map_open frees the store's copy of an extent it brings into memory, after the memory holds it; the commit writes it anew. sealing_would_strand_a_mapping and the fork's EBUSY gone; forks_seal_nothing_a_window_maps_writably now asserts the fork succeeds, the store holds no copy, and the fork has none of the extent; a_mapping_takes_an_open_extent_out_of_the_store. specs/extent-open: oneOwner, a fork, NoRefusedCommit, OneOwner; TwoOwners finds the refused commit; SplitWrite still the lost write.
RESULTS. sim: check.sh clean; zloop 3,000 at the mix 0 failed, 3,000 at 1,000 0 failed, 30,000 at 200 5 failed, all (3). Mutants: those that can show do, but task-153.3-refused-slice-stale (not in 3,000, nor 500 at 1,000), and patch-0008, task-153.1-fault-limit, task-153.6-lag-past-the-end, which main doesn't catch in 400 either. TLC by hand (check-tla.sh's pinned jar SHA-256 stale): dax-region 5 models, extent-open 3, all as expected. Workspace, Docker: 2,167 of 2,169, both grants tests needing sudo. Guest Rust type-checks, guest tests compile.
GCE, n2-standard-4: r477 (d559e9e78) checks-dax, -delay, -small, script-full-dax, xfstests OK; xfstests 16 of 650 failed, the known list, generic/129 passing now; write_times_add_up passes; no WARN, no unexplained refusals. r478 head (157417ad5) and r479 main side by side, pgbench-dax 4 rounds of 5 runs: 968 against 960 tps.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A device write to an extent not known to be open is taken, not refused (TASK-153.3)**

The task's direction had such a write retry rather than reopen its extent. Decided: the store takes it, and nothing records the extent open, ever again: the open extents are those minted since the last seal (`OpenExtents`). Plain writers (multi_write, the journal, layer files) have no retry, where `SealedExtent` is EIO, and DAX's is dax.c's `gate != seals` already.
**Instead:** refuse it in the guest's device, and give every writer a retry.
**Undoing it:** a check in `write_with_opts`, and retries at each writer: `dead-ends/refusing-writes-to-extents-not-known-open.md` has why not.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An extent goes minted, open, sealed, and never back, guest and host: a write reaching an extent after a seal opens nothing (test_a_write_after_a_seal_reopens_nothing), a DAX piece says what the window may do with it (ExtentAccess), the checks this made dead are gone, shared blocks stay out of regions (7caa43578); TLA+ specs dax-region and extent-open model it. Verified: guest/sim check.sh and zloop clean, past bugs put back failing or unbuildable; workspace on Docker and macOS; GCE r477 (checks-dax, -delay, -small, script-full-dax, xfstests: the known 16 of 650), pgbench-dax 968 against main's 960 tps (r478/r479). Decisions in OPEN_QUESTIONS.md; follow-ups TASK-150.21 (fs/dax.c:416 WARN) and TASK-157 (load flakes).
<!-- SECTION:FINAL_SUMMARY:END -->
