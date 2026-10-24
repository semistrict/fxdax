---
id: TASK-79.78
title: >-
  A fork whose acknowledgement fails is deleted locally though its PUT took
  effect: the collector keeps its orphan's inheritance for ever
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-24 02:46'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 336000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.72 (round 2a). When Remote::forked fails, ExtentStore::fork (crates/fxdax/src/extents.rs) deletes the child locally, even if the fork's segment PUT took effect (only the acknowledgement failed, or a dead writer's PUT lands later). The parent's next writer then gives the deleted child a branch record; the collector treats that orphan as live and keeps what it inherited for ever.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fork whose PUT took effect is either kept or deleted remotely too, whatever failed after it
- [ ] #2 The collector frees what an orphaned fork inherited once nothing else holds it, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 2c: a failed fork's cleanup that stops partway leaves the fork's handle (deleting it finishes the deletion), so this path makes no orphan; unfork failing is tested too. AC #2 is not met as written: the collector cannot tell a branch no host holds from one held idle somewhere, so orphans made before the fix are not reclaimed (TASK-79.85). Child-ID ownership, which unfork and the leftover handle's deletion rely on, is TASK-79.82.

Finalization check (after the push of 47b7a348): not done. AC#1 is proven only for failures from Remote::forked on: a failed acknowledgement, a later writer taking over, a crash during the cleanup, or the remote deletion failing. It is not proven for failures inside Store::fork after its commit (the flush, framing or record write). ExtentStore::fork then returns without unfork while the footer still uploads. A crash between the commit and the record write leaves a handle that delete_host_with skips. Both can still leave a branch in object storage that no handle names (round2c review should-fix, 938b2c2c's own note). That path is TASK-79.82's AC#3. AC#1 can be checked once that is fixed, or if the user scopes AC#1 to Remote::forked's failures. AC#2 is not met as written, as the Implementation Notes say. No test shows the collector freeing what a true orphan inherited, meaning a live record with no handle on any host. The crash test only shows the collector freeing it after the leftover handle is deleted. The collector cannot tell a branch no host holds from an idle one, so orphans made before the fix are never reclaimed (TASK-79.85). Closing AC#2 needs TASK-79.85, or the user restating AC#2 as round 2c's review suggests: a fork whose PUT took effect leaves no branch that no handle names, and deleting the leftover handle lets the collector free what it inherited, pinned by a test. Audit: No AC is claimed, and the DoD claims hold: the named tests pass on both platforms, check exits 0, and nothing ported is touched. I agree it is not done. extents.rs:829-848 shows that when fork_host fails after its commit, ExtentStore::fork returns through `?` without unfork. fork_host's inspect_err removes the handle, so a footer that uploads later leaves a branch no handle names. 938b2c2c says so, so AC#1 is open, which is TASK-79.82 AC#3. For AC#2, no test shows the collector freeing what a true orphan inherited: the crash test frees it only after the leftover handle is deleted. The Implementation Notes and TASK-79.85 say orphans are never reclaimed.

Landed with TASK-79.82 (3ca2e731): a fork failing after its commit keeps its handle and is cleaned up as a Remote::forked failure, object storage included; a handle naming a branch the lineage lacks (a crash between the commit and the record) deletes the claimed child in object storage too. AC #1 holds. AC #2 (the collector freeing an orphaned fork's inheritance) is not shown: the fix stops orphans being made rather than collecting them.
<!-- SECTION:NOTES:END -->
