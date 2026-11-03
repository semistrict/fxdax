---
id: TASK-79.82
title: >-
  A failed fork deletes whatever branch has the child's ID, though the guest
  chose it: another live branch on the host can be deleted
status: Done
assignee: []
created_date: '2026-10-23 09:16'
updated_date: '2026-11-03 13:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 341000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.78 (round 2c). A FORK's child ID comes from the guest (extentd/server.rs, args[0]). When Remote::forked fails, Remote::unfork (crates/fxdax/src/remote.rs) calls delete_branch(child, token), and branch::delete returns Stale only when the current record's token is greater (extent_remote branch.rs), so a branch held at the same host's token gets a FINAL lease and its record marked deleted. record_fork failing with Corrupt 'has another origin', which is what a child ID belonging to another live branch gives, is exactly what triggers unfork. So a fork naming another branch's ID deletes that branch. A leftover fork handle deleted later (host_store::delete) deletes by ID the same way. A footer naming another branch as a fork also bricks the parent: every writer's record_forks then gives Corrupt for ever. Related, from TASK-79.78's review: Store::fork's failures after its commit (flush, framing, the record's write) return without unfork, and a crash between the commit and the record write leaves a handle naming a branch the lineage lacks, which delete_host_with then skips; both fixes delete by ID and need the ownership check first. Proposal from round 2c: with a remote, ExtentStore::fork refuses (EEXIST) a child whose record already exists in the bucket, before forking (one GET on every fork's latency path), and unfork and `before` delete only a branch whose record is absent or names this fork's origin. Needs a decision on who owns child IDs; touches extentd (TASK-42's area).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fork can never delete or mark deleted a branch it did not create, whatever ID the guest gives
- [x] #2 A fork to a child ID another branch has fails with EEXIST and leaves the parent's footers and records sound
- [x] #3 Failures after Store::fork's commit, and a crash before its record is written, leave nothing a later deletion skips or mis-deletes, pinned by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
User decision (2026-10-23): with a remote, a fork refuses (EEXIST) a child ID whose record already exists in the bucket, before forking (one GET per fork); unfork and a leftover handle's deletion delete only a branch this fork made (record absent or naming this fork's origin).

Landed (3ca2e731): with a remote, a fork claims its child ID first (one GET; EEXIST if a record exists or the store has a claim for it), records its origin in a claim file before its segment goes, and unfork and a leftover handle's deletion delete a child in object storage only if its record is absent or names that origin (branch::delete_fork). AC #2 and #3 hold (forks_to_a_branch_the_bucket_has_fail_with_eexist, forks_failed_leave_branches_that_took_their_childs_id_be, forks_failed_whose_handles_are_deleted_later_leave_branches_that_took_their_childs_id_be, forks_failed_once_committed_leave_their_child_no_branch, forks_whose_host_crashed_before_their_record_are_deleted_in_object_storage_too, host::tests::forks_failed_once_committed_keep_their_handles). AC #1 is not fully met: another branch taking the ID after the GET and before a failed fork's deletion takes the child's lease can still be deleted (documented on delete_fork). Still EIO, not EEXIST: a fork to a branch the local store has when object storage has nothing. macOS: workspace 2493/2493 and every Shuttle suite; Linux (GCE fxdax-test-r8m): workspace 2571/2571, Shuttle suites, and the guest cases fxfs-guest, fxfs-guest-dax, fxfs-guest-checks, fxfs-guest-checks-dax, extent-guest, fxfs-guest-pgbench and fxfs-guest-pgbench-dax, all OK, with a guest kernel built from the tree; scripts/upstream-changes.py --check passes. Pushed 430bb064..3ca2e731.

Handed over (2026-10-24): landed on main; what is left is in the notes above.

2026-10-29: AC1's race closed for forks whose segment went (an origin to claim the ID with): branch::delete_fork gives a child with no record the record its fork would ([record_fork]'s, live, naming the origin) by a create that succeeds only if none is there, before ending the child's lease; a branch taking the ID after the deletion found it free has its record there first, the create fails, and the deletion leaves it be (test forks_deleted_leave_a_branch_taking_their_childs_id_meanwhile, failing before: the other branch's lease ended, its record marked deleted). The claim is live, not deleted, so a deletion refused by a later placement (ESTALE at the lease) changes nothing wrong (forks_failed_whose_deletion_failed_keep_their_handle_and_branch); a child so claimed has its lease ended and record marked deleted alone (delete split into end_lease and mark_deleted), as delete treated a branch with no record, without recovering it (stores_are_deleted_while_their_forks_live_locally recovered a fork whose parent was gone otherwise: 'lost'). Remaining: a fork whose segment never went names no origin, and has none to claim the ID with. macOS scripts/test.sh passed (1826); GCE Linux workspace r246 pending.

GCE r246 (main 3866a31d6): Linux workspace 1955/1955 and the Shuttle suites, fxfs-guest-dax passed.

2026-11-01, branch test/fork-faults, each fix failing its test with the bug put back: delete_fork takes the child's ID with a record before ending its lease, naming no origin too (sim seed 0: another branch's lease ended; rule fork_claims_ids); a record is live only if the parent's branch holds the fork's segment (seed 37: 'collector: lost', every pass and the deletion's resumption failing; rule fork_claims_landed); record_fork's ChildTaken: writers go on, the fork fails with EEXIST, where every writer of the parent failed for ever; the collector recovers a child by its record's origin, where it lost what the child inherited; a handle whose child's ID another took refuses that branch, where it fenced it and uploaded into it; a child taking a record of its own (create_forked) keeps its claim and takes none of another's; a fork to a branch the store has gives EEXIST, not EIO. AC1 NOT ticked, one case left: two forks on different hosts, or of different parents, naming the same ID within one round trip, neither segment landing, and both children then taking records of their own: create_forked's bytes are the same for both, so each takes the other's for its own, and deleting one ends both. A record has nothing unique to tell them apart without a format change. A plain branch, and any fork whose segment went, is proved safe by the simulation, the sweep and the tests above. Not landed; not run on GCE.

Handed back 2026-11-02 (triage): the test/fork-faults work is all on main (707b8cb75 .. 894b4c1a1); the 'not landed' note is stale. Left (AC1): two forks, on different hosts or of different parents, can name the same child ID within one round trip with neither segment landed; both take identical create_forked records, and deleting one ends both. Needs a record format change, likely with TASK-153.9's branch owner.

2026-11-03, AC1's last case closed by TASK-153.9: every record names its branch's owner, chosen by the fork's host and carried in its footer and claim, and records are compared by owner (Record::of_fork). Two forks naming one child, neither segment landing, no longer both take the child's record; deleting one leaves the other's. Tests: branch::forks_to_one_child_take_one_record; simulation action twins in failed_forks_deleted_anywhere, and forks_to_one_child_need_records_owned (seed 0 with the rule off). GCE fxdax-test-t1537-r1/r2 and macOS pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fork can no longer delete or take a branch it did not make: claims, records by owner, and the collector's handling of claims (see notes). The last case, two forks naming one child within a round trip, closed by records naming their owners (TASK-153.9); shown by the simulation's twins with the rule off.
<!-- SECTION:FINAL_SUMMARY:END -->
