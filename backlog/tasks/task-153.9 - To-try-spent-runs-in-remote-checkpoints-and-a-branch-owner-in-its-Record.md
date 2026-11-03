---
id: TASK-153.9
title: 'To try: spent runs in remote checkpoints, and a branch owner in its Record'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 13:39'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 553000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Written-once state lives on one host while the bucket is the truth (TASK-79.6, 150.6), and branches have no owner (TASK-150.4). Both format changes; TASK-150.6 needs the first. Bundle with the collector change in one format bump.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A format change designed for them together
- [x] #2 TASK-150.6 fixed by it, its test red first
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (2026-11-03). Spent runs in remote checkpoints landed with TASK-150.6 (State.spent, replayed from footers' frees, kept in checkpoints). Left: a branch owner in its Record.

What 'owner' must give: TASK-79.82's last case is two forks naming one child ID within a round trip, neither segment landing; both take create_forked records of identical bytes, and bucket::create, reading back what it finds, takes the other's for its own; deleting one ends both. Every create-only write's bytes must be unique to its writer, as segments' (epoch) and leases' (holder) are; records are not.

Decided: Record.owner, a number the branch's maker chose at random, unique to that making, which every record of the branch names: whoever writes it. For a fork it is chosen by the fork's host when it claims the child (the claim file names it) and carried in the fork's footer (Footer.forks: Vec<Child { branch, owner, point }>, Footer.points folded in), so that the parent's writers, its next writer and its deletion write the same record (record_fork), and the host's own paths (create_forked, fork_point, delete_fork) the same again. A record is a fork's iff it names the fork's owner (Record::of_fork(owner), replacing the origin comparison): delete_fork, record_fork, fork_point and fxdax's attach compare owners. A plain create's owner is random: two hosts creating one ID no longer both succeed.

Not a tenant: tenants stay authorized by their directory and the VM manager's CreateStore (TASK-154). A tenant name in the bucket would be a cross-host claim nothing authenticates, and honouring a tenant's named branch again is the check TASK-150.4 removed. Recorded in OPEN_QUESTIONS.md.

1. extent_frame: Child, Footer/Contents.forks of children. extent_store: forks carry their Child; FORMAT 5.
2. extent_remote: Record.owner; every record creation takes an owner; comparisons by owner; rule records_owned.
3. fxdax: the claim file names the owner, written whole or not at all; fork_durable picks it.
4. Simulation: action twins, two forks naming one child, both taking records of their own, the loser deleting its fork: test_needs!(.., records_owned, ..) red with the rule off. TASK-79.82 AC1 closed by it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented, as planned. extent_frame: Child { branch, owner, point }, Footer/Contents.forks of children (points folded in). extent_store: Store::fork_with and fork_host_with take a Child; FORMAT 5. extent_remote: Record { format, owner, .. }, Record::of / decode / of_fork(owner); create, create_forked, record_fork(Child, origin), fork_point(.., owner), delete_fork(.., owner, ..); a deletion's own record is of its holder; State.forks carry owners (checkpoints too). fxdax: the claim file names the owner first ('owner <hex>'), written whole or not at all (a file linked into place); fork_durable picks the owner at random.

Simulation: action twins (two forks naming one child, both create_forked, the loser deleting its fork, which must leave the winner be); forks_to_one_child_need_records_owned finds two records taken for one child at seed 0 with records_owned off. Every owner in the simulation is real (forks, restores, unforks, contests). Unit tests: forks_to_one_child_take_one_record, forks_delete_only_their_own_branches (owners), a_child_gets_one_record. Tenants not put in records: OPEN_QUESTIONS.md # Decisions.

Verified: macOS scripts/test.sh at e8e186485: workspace 2051/2051, Shuttle 61, 7, 6, 21, 10, 18, scripts 135 OK; the extent_remote simulation 37/37. GCE fxdax-test-t1537-r1 (us-east4-a, n2-standard-4): Linux workspace 2215/2215 and the same Shuttle suites; its guest stage stopped at a test_cases host test (XFS loop umount busy, no fxdax code), so r2 ran the guest cases alone: fxfs-guest-dax, -live-fork, -migrate-dax, -migrate-tenant-dax, -snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -suspend-dax all OK, test_cases 53/53. Both hosts deleted. scripts/upstream-changes.py --check passes.

AC2: TASK-150.6 was fixed by this design's first half (spent runs in the branch's state and checkpoints), landed with TASK-158; its test extents_freed_are_spent_on_the_host_that_takes_their_branch was red without the spend (150.6's notes) and passes in this run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Spent runs are in remote checkpoints (TASK-150.6, landed with TASK-158). Branch records now name an owner, chosen at random by the branch's maker and carried in fork footers and claim files, so every record of a branch is the same bytes and no two makers' are: TASK-79.82's last case is fixed, and the simulation's twins action finds it at seed 0 with records_owned off. Owner is the maker, not a tenant (OPEN_QUESTIONS.md). Verified macOS and GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
