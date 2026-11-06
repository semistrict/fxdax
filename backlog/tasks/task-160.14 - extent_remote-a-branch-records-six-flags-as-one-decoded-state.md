---
id: TASK-160.14
title: 'extent_remote: a branch record''s six flags as one decoded state'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-06 06:56'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 585000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Record's origin/forked/deleted/point/forking/claimed flags act as an unwritten state machine: built by hand at 7 sites, interpreted in four modules, each create-then-look site with its own whose-record test, the Lost-is-fine rule written twice, five read-modify-write loops (take_lease and end_lease near-identical). Suspected, unconfirmed: record_fork accepts any record of an equal origin, so a direct fork and a fork of a point at one segment, given one guest-chosen ID, may take each other's. Explains TASK-79.82 (9+ fixes), 79.78, 79.84, 158. Proposal: enum Kind decoded once, refusing illegal combinations, and one branch::transition CAS loop for every create and update. The byte layout with TASK-153.9.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Records decoded once; every create and update through one transition
- [x] #2 A table test over all 64 flag combinations
- [x] #3 Sim profiles pass, each rule still switchable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Suspected bug: unit test, a direct fork and a fork of a point at one segment, one child ID, two owners; red with records_owned off (what the format bump's owner fixed), green on main. Plus a class test: every pair of record makers of one ID, of two owners, takes nothing of the other's.
2. record.rs: Record { format, owner, kind: Kind, deleted }, Kind { Root, Own, Fork(Origin), Point(Origin), Forking { origin, point }, Claim(Origin) }, decoded once from the wire's flags (unchanged bytes) through serde try_from, refusing illegal combinations; queries writable, is_child_of, may_drop, origin; branch::lost_is_ok for the Lost-is-fine rule.
3. branch::transition: one CAS loop (get, step, create or conditional update, again) that every record and lease create and update goes through: create, create_forked, record_fork, fork_point, finish_point_fork, abandon, delete, mark_deleted, delete_fork, take_lease, end_lease.
4. ForkedAt goes: delete_fork takes the Kind its child's record is or would be; fxdax's Claim holds a Kind once its segment goes.
5. Table test over all 64 flag combinations. Sim rules stay inside the transitions.
6. Proof: all test_needs!/test_holds! profiles, SIM_SOAK of restores and forks_and_frees, scripts/test.sh, GCE test-gce.sh all with guest snapshot/migrate/fork cases.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Claims re-checked on main after the format bump (3415eb074): the flags were an unwritten state machine (6 kinds, 12 legal records of 64 combinations); records were built by hand at 9 sites, all in branch.rs, and read in branch.rs, writer.rs, collector.rs and fxdax remote.rs (plus recovery and the sim). The bump had already unified the whose-record test to Record::of_fork(owner), though each site kept its own rules around it. The Lost-is-fine rule was written twice (delete, collector recover_live); there were five read-modify-write loops (finish_point_fork, abandon, take_lease, end_lease, mark_deleted), with take_lease and end_lease near-identical; ForkedAt, fxdax's Claim (Forking + origin) and delete_fork's live/point/claimed were three copies of one classification.

Suspected bug: real until TASK-153.9. record_fork used to accept any existing record of an equal origin; since the bump it accepts only a record naming the fork's owner, which is random per fork. forks_and_forks_of_points_at_one_segment_take_no_records_of_each_other passes on main and is red with records_owned off: there both forks take the one record, and the loser's cleanup deletes the winner's branch. The class test makers_of_one_id_take_nothing_of_each_others covers every maker of a record against every other maker of another owner. Structure: the class is now one query (Record::is_child_of) behind one transition.

Evidence: extent_remote suite 158/158, with every test_needs!/test_holds! profile; SIM_SOAK=25 of restores (points_restored_on_many_hosts_at_once) and forks_and_frees, 5,000 seeds each, pass; scripts/test.sh on macOS passes (2,057 workspace tests, Shuttle suites, scripts' tests); GCE fxdax-test-t16014-records (us-east1-b, deleted): workspace 2,228 passed, plus the Shuttle suites, and guest cases fxfs-guest-live-fork, -snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -migrate-dax, -migrate-tenant-dax, 7/7; scripts/upstream-changes.py --check passes.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**A branch record's flags stay on the wire; it is decoded once into a kind (TASK-160.14, 2026-11-03)**

A record's six flags (origin, forked, deleted, point, forking, claimed) were read as flags in four modules. Decided: they stay the bytes in the bucket, unchanged (FORMAT 1), and are decoded once into `record::Record { format, owner, kind, deleted }`, with `Kind` one of Root, Own, Fork, Point, Forking, Claim. Any other combination is refused as corrupt (52 of the 64). What a record allows is a query on it (`writable`, `is_child_of`, `may_drop`, `branch::lost_is_ok`), and every create or update of a record or a lease goes through one compare-and-swap loop, `branch::transition`. `branch::ForkedAt` is gone: `delete_fork` takes the `Kind` its child's record is or would be, and fxdax's claim holds that kind once its fork's segment goes. The suspected bug (a direct fork and a fork of a point at one segment, given one child ID, taking each other's records) was real until TASK-153.9's owners; a test now shows both that it no longer happens and that it does with `records_owned` off.
**Instead:** put the enum itself on the wire (serde's tag), so that no illegal record can be written at all; but decoding already refuses them, and the flags are what doc-7 describes.
**Undoing it:** fold `record.rs` back into `layout.rs` with public flags; nothing on the wire changes either way.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A branch's record is decoded once (record.rs) into Record { format, owner, kind, deleted }. Kind is one of Root, Own, Fork, Point, Forking or Claim, and the wire's flags are unchanged; the 52 flag combinations that say no kind are refused as corrupt. Queries on the record replace the flag reads in four modules. Every record and lease create or update goes through one branch::transition compare-and-swap loop, with the sim's rules kept in each step. ForkedAt is gone: delete_fork takes a Kind, and fxdax's claim holds one. The suspected record_fork bug was real only before records named their owners (TASK-153.9); a test shows that, red with records_owned off. Verified by the extent_remote suite with every sim profile, soaks of restores and forks_and_frees, scripts/test.sh on macOS, and GCE (the workspace plus the snapshot, migrate and fork guest cases). Commits b781ff302 and dcf9023e0.
<!-- SECTION:FINAL_SUMMARY:END -->
