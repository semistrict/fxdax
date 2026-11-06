---
id: TASK-186
title: >-
  fxdax: claim_origin reads a child's claim, then writes it: it revives a claim
  the child's deletion removed meanwhile, or names a dead fork's origin in a
  later fork's claim of the ID
status: Done
assignee: []
created_date: '2026-11-05 16:24'
updated_date: '2026-11-06 01:44'
labels:
  - remote
  - structure
dependencies: []
priority: medium
type: bug
ordinal: 641000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found looking around TASK-184 (2026-11-05). crates/fxdax/src/remote.rs: claim_origin (the parent's uploader, for each child a gathered segment's footer names) reads the child's claim and writes it back with the origin, then InFlight::upload writes the child's marker if it was claimed. Nothing orders that with the main thread's writers of the same child's files. (1) A fork failing once its commit was made (ForkCommitted) deletes its child here (host_store::delete_with's after: unmark) while the uploader, alive, may gather the fork's segment on its interval: read before unmark, written after, the claim and the child's marker come back for a branch the store no longer has; a later fork to that ID here finds it claimed (EEXIST) and the files stay until the store goes. (2) A segment of a failed fork to child X, never uploaded (its child deleted), is gathered after a later fork to X claimed it: claim_origin names the dead fork's origin in the new fork's claim, though the footer's fork has another owner (Child::owner), and the new fork's own segment keeps it (Forked::Made is kept). Deleting X then goes by the wrong origin. TASK-184 made each write's new file its own, which ends the shared-temp ENOENT, not these.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test deleting a child as the uploader names its fork's origin (failpoint remote::written) leaves no claim or marker of it, red now
- [ ] #2 claim_origin names an origin only in the claim of the fork whose owner the segment's footer names; a test with a dead fork's segment gathered after a later fork to the ID claimed it, red now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Moot after TASK-165.7 (branch task-165-7, commit eaeffc8b4): fxdax writes no claim file any more (claims, origin-* files, claim_origin, disclaim are gone); a fork's child is recorded in the bucket alone. The race this task describes has no code left to happen in.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as moot: TASK-165.7 removed fork claims (claim_origin and its read-then-write) altogether; nothing to fix.
<!-- SECTION:FINAL_SUMMARY:END -->
