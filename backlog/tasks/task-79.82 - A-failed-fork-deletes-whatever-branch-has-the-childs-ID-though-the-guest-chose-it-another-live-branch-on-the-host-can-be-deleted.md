---
id: TASK-79.82
title: >-
  A failed fork deletes whatever branch has the child's ID, though the guest
  chose it: another live branch on the host can be deleted
status: To Do
assignee: []
created_date: '2026-10-23 09:16'
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
- [ ] #2 A fork to a child ID another branch has fails with EEXIST and leaves the parent's footers and records sound
- [ ] #3 Failures after Store::fork's commit, and a crash before its record is written, leave nothing a later deletion skips or mis-deletes, pinned by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
