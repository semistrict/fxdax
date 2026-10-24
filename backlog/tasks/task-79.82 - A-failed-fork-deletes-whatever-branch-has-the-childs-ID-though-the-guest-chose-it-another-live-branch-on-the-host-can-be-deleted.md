---
id: TASK-79.82
title: >-
  A failed fork deletes whatever branch has the child's ID, though the guest
  chose it: another live branch on the host can be deleted
status: In Progress
assignee: []
created_date: '2026-10-23 09:16'
updated_date: '2026-10-24 12:29'
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
- [ ] #1 A fork can never delete or mark deleted a branch it did not create, whatever ID the guest gives
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
<!-- SECTION:NOTES:END -->
