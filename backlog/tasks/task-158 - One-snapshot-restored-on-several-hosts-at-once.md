---
id: TASK-158
title: One snapshot restored on several hosts at once
status: Done
assignee:
  - '@claude'
created_date: '2026-11-02 02:53'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 565000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-154: a host copy of a point is a writer of its branch, and each restore fork commits there, so once one host forks it every other host copy, the source too, fails EIO until DropStore and CreateStore. Predates that branch, affects the default tenant too, and rules out one snapshot restoring on several hosts at once. Points should be read-only ancestors. Needs a protocol change. Pinned by copies_of_a_point_another_host_forked_are_taken_anew.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decided whether several hosts at once is needed
- [x] #2 A point restores on several hosts at once, or the limit is documented for the VM manager
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-01, the user: yes, one snapshot must restore on several hosts at once, and that it doesn't is a basic design flaw. Formats may change, no migration (AGENTS.md).

## Design: a point is a record, read by any number of hosts, written by none (2026-11-02)

Why a fork commits on its parent today. Two reasons only. (1) To name a place: Origin{parent, at} needs a segment of the parent's, and the child's state is the parent's as of it. (2) So that the fork is linearised against the collector: the parent's acknowledged segment names the child (state.forks), and the child's record is made after it; a pass finds the child by its record or by the parent's state. Neither needs a commit on a branch whose state never changes: a state that never changes has its place already, and what keeps it alive is a live record naming it.

So: a point is born sealed. The fork that makes it (FORK with FORK_QUIESCENT, what a snapshot's take_snapshot sends, no VMM change) is an ordinary fork of the VM's store S, a commit of S's, whose footer lists the child in `points` too. Its Record is {origin: S@at, point: true}, from the first: there is no state in which it is a writable branch. It never has a lease, a segment or a checkpoint. Its state is S@at, for ever. A point is a durable name for S@at.

A restore's fork of point P to child C on any host writes one object, C's record, naming P's origin itself, S@at (recovery, the collector's marking and state_at need no change: C is to them a fork of S at `at`), and nothing of P's or S's.

### States of a fork of a point
Record of C: absent -> forking (`forking: Some(P)`, origin S@at, live to the collector, nothing may rely on it) -> made (`forking: None`) -> deleted. forking -> deleted too, by the fork's cleanup or the collector's abandoning it.

### Steps on host H (H has a copy of P: a store that took P's state read-only, recovered from the bucket, no lease)
1. claim: GET C's record (absent or EEXIST), then claim file in the store's directory naming P and the origin, durably.
2. local fork: handle, then Store::fork_with, a commit in H's local copy alone. P's copy has no uploader, so the commit goes nowhere. C is a local branch over P's layer.
3. R3: create C's record, forking.
4. R4: read P's record. Deleted: the fork fails. Else
5. R5: conditional update of C's record, forking -> made. Lost to a deletion or an abandoning: the fork fails.
Only then does the fork return. C's writer takes C's lease on its VM's OPEN, as any branch's.

### Who writes what
- S's writer: S's segments, as today; P's record, or S's next writer from state.forks, or P's first attach (record_fork, with `point`).
- H, restoring: C's record only.
- Nobody: P's segments, checkpoints, lease. Writer::take refuses a point (Error::Point) before it touches the lease, so that no path can: a VM's OPEN (EROFS), take_parent, a stale copy.
- The collector: as before, plus abandoning: it marks deleted a forking record whose point is deleted, by a conditional update.
- DeleteStore(P): end_lease, mark P's record deleted, as any branch. Children of P keep what they need by their own records.

### What the collector relies on
need(C) = need(S@at) = need(P), by construction, so a live P keeps all C needs. A pass keeps need(C) if it finds P live or finds C (forking or made).
Invariant: if a fork returned, no pass deletes or leaves out of its moves any of need(C) while C lives.
Proof. Let Q be a pass. need(C) is at risk from Q only if Q treats neither P nor C as live. Three cases, by what Q saw of P's record.
(a) Q read P live: it recovers P, keeping need(P) = need(C).
(b) Q read P deleted. The fork returned, so R4 read P live, so R3 < R4 < mark(P) < Q's read of P. Q lists the records again after reading a new one, so it finds C. Reading C forking, it abandons it by conditional update: if that won, R5 failed and the fork never returned, a contradiction; if it lost, C is made, and live. Either way Q keeps need(C).
(c) Q never saw P's record: P's record came after Q's listings. This case my first proof left out, and the simulation found it (seed 5310, restores): P forked, restored, deleted, S freeing what P held and a checkpoint dropping the fork, all between Q's listing and Q's reading S's state. So Q lists the records again after it reads the states. A fork leaves a state only once its record is deleted (Writer::checkpoint), so if Q read a state without the fork, that listing finds P's record, and (b) applies to it. If Q read a state with the fork, it queues P (forks_from_states), and (a) applies. If P's fork came after the state Q read, need(P) is what that state holds, or objects newer than Q's listing of candidates. Q goes on listing only while what it recovers has a checkpoint of its own, as only those states can have dropped a fork: so a steady rate of restores, whose children have none, can't keep a pass going for ever.
The same hole was open for forks of forks with a deletion in the middle, before points.
Unacknowledged forks: C forking with P deleted: nothing relied on it. The pass abandons it before recovery, so a Lost can't stop the pass. An abandoned C has no segment, no lease, no writer. No Lost is ignored anywhere: a live made branch whose recovery fails stops the pass, as today.
Order of objects within a pass is unchanged: candidates are listed first; a fork writes a record alone, never a candidate.

### Host death at each step
1. before claim: nothing.
1-2. claim, handle, maybe local branch: deleting the handle: delete_fork(OfPoint) writes a deleted record, so a PUT of the dead host's landing late finds it taken. Opening the handle, if the local branch exists: attach finishes the fork (R3-R5), or fails if P went.
3-4. forking record, host dead: opening finishes it; deleting deletes it, without recovering it (it may name what is gone); neither, disk lost: an orphan, which keeps nothing P doesn't, until P goes and the collector abandons it, or `fxdax-gc --delete-branch`.
5. made, reply lost: the handle opens as a fork made.
A dead host's R3 landing late, after its cleanup: finds the deleted record. Its R5 landing late: conditional, fails.
Making the point itself, S -> P: an ordinary fork, all of whose cases TASK-79.x covers. One new one: P's fork never landed, its handle left: a claim says it is a point, attach refuses (no origin, no point), the handle is deleted. It takes no branch of its own.

### A copy of a point on a host
CreateStore(branch): as before; on opening, the store finds the record says point: recovers the state, adopts it if empty (Plan::Take, local commit), or checks it holds it; the Fetcher reads from that state. No lease, no fence, no token, no uploader loop; ask/uploaded return at once. Immutable, so a copy is never stale: the EIO, ESTALE and DropStore-then-CreateStore dance goes. A VM's OPEN of a point as writer: EROFS (was: it could write its snapshot). Followers may read it. A deleted point: forks fail ENOENT, children live on.

### What stays
Forks of anything that is no point (a VM's store, ForkStore, an imported base) remain commits of their branch, one host at a time: a question for the user (OPEN_QUESTIONS.md), not needed for snapshots.

### Format
Footer/Contents: `points`, those of `forks` that are points. State.forks: Fork{child, at, point}. Record: `point: bool`, `forking: Option<u64>`. Claim file: says point / of a point. No migration (AGENTS.md).

### Rules for the simulation, each with a test turning it off
points_take_no_writer, forks_check_points (R4), forks_pending (R3's forking state), collector_abandons.
A failed fork's claim (found by the simulation, seed 3184, an older bug than points): delete_fork made a live record from holds(), but a segment of that number needn't be the fork's, and the record may land, a dead host's, after a pass took what it names: Lost for ever, every pass stopping. The record now says `claimed`: Lost on a claim no writer has deletes it and the pass goes on. Any other Lost stops the pass, as before.
A point's own failed fork gets a deleted record from the first (seed 233): no placement refuses that, and a stale copy finds no live record naming a segment that may not be its fork's.
Not folded in: TASK-153.7 (a fork of a point declares itself by its record, which needs no more); TASK-153.9's owner (a VM manager's CreateStore stays the grant, nothing here needs it).

## Finished, 2026-11-03

The WIP (71cd7b897) rebased onto main (dcbdd6247) as three commits: TASK-150.6's spent IDs on its own (it rode in the WIP); points; ForkStore's `point`. Formatting-only hunks the WIP made in files it didn't otherwise change were left out: main isn't rustfmt-clean, and they were no part of this. One hunk of extents.rs taken by hand: main had dropped sealing_would_strand_a_mapping.

Decided (OPEN_QUESTIONS.md # Decisions): imported bases fork on several hosts too. ForkStoreRequest.point, and fxdaxctl point SRC DST, make a point of any store once nothing is open; each host forks a copy of it, as a snapshot's restores do. A point of a point is refused, EINVAL / INVALID_ARGUMENT. Test: grants.rs bases_made_points_fork_on_several_hosts_at_once, 3 hosts x 3 forks at once. The stale gcloud question wasn't on main; nothing to remove.

Verified:
- macOS, scripts/test.sh at the rebased tree: workspace 2027/2027, the Shuttle runs (60, 7, 6, 21, 10, 18), scripts 135 OK.
- GCE fxdax-test-t158-r1 (us-central1-a, n2-standard-4), scripts/test-gce.sh all, with fxfs-guest-migrate-tenant-dax, migrate-dax, snapshot, snapshot-dax, snapshot-volumes, snapshot-volumes-dax, suspend-dax: workspace 2189/2189; the same Shuttle and follower runs; all 7 guest cases OK. migrate-tenant-dax restores one snapshot four times at once (three copies on hosts of their own, and the snapshot's own). grants.rs: copies_of_a_point_another_host_forked_restore_on (the old pin, copies_of_a_point_another_host_forked_are_taken_anew, inverted), several_hosts_restore_one_snapshot_at_once, vm_processes_write_no_point, points_restore_whatever_placement_took_them, bases_made_points_fork_on_several_hosts_at_once: pass. restore_faults: all 8 pass.
- The simulation: points_restored_on_many_hosts_at_once at SIM_SOAK=100 (20,000 seeds, release): no violation. Each of its rules turned off is found: points_need_refuse_writers, restores_need_points_read_after_their_records, restores_need_records_saying_they_are_being_made, collection_needs_abandoning_restores, collection_needs_records_listed_after_states.
- scripts/upstream-changes.py --check: passes.
- An earlier GCE run (fxdax-test-t158-wip) staged a tree mid-edit and didn't compile: no evidence either way, deleted. fxdax-test-r510 no longer existed.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Bases fork on several hosts at once too, as points (TASK-158, 2026-11-03)**

A snapshot's point now restores on any number of hosts at once. A fork of anything else, such as a base from `ImportImage`, is a commit on its branch, made by its writer, one host at a time. Decided: `ForkStoreRequest.point` (and `fxdaxctl point SRC DST`) makes a point of any store, once nothing is open, and each host forks a copy of it (`CreateStore` of its branch), as a snapshot's restores do. The base stays writable on its host; what it writes after needs a new point to reach forks. A point of a point is refused (`INVALID_ARGUMENT`). `docs/integrator-guide.md` says how.
**Instead:** bases fork on one host at a time, and the guide says so.
**Undoing it:** drop the field and `ExtentStore::point`'s use from `stores::fork`; nothing else depends on it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One snapshot restores on any number of hosts at once. A point is a record read by any number of hosts and written by none: it has no lease, segment or checkpoint, and a restore's fork writes only its child's record (forking, then the point read, then made, with a condition). The collector abandons forks being made of deleted points and lists the records again after reading the states; a failed fork's record says it is a claim. Any store, a base included, is made a point by ForkStore's point, so bases fork on several hosts at once too. Verified: macOS scripts/test.sh (2027/2027 and the Shuttle runs); GCE fxdax-test-t158-r1, workspace 2189/2189 and 7 guest cases, fxfs-guest-migrate-tenant-dax restoring four times at once; grants.rs with the old pin inverted; the restores simulation at 20,000 seeds, and each of its five rules shown needed.
<!-- SECTION:FINAL_SUMMARY:END -->
