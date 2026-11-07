---
id: TASK-204
title: >-
  A store taking a branch over from a writer still uploading fails for good
  (Displaced, EROFS) when that writer's segment lands past its fence: doc-7 says
  it takes the branch again, keeping its guest
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 06:55'
updated_date: '2026-11-07 09:26'
labels:
  - storage
  - takeover
dependencies: []
priority: high
type: bug
ordinal: 646000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the storage review of 2026-11-06. doc-7, Fencing: "A create that fails on an earlier writer's segment, landed late where its own was to go, means its later segments are void: it takes the branch again, which keeps its guest." The code stops instead: Writer::upload returns Error::Displaced and stops the writer for good (writer.rs:356-362, writers_displaced_by_an_earlier_writers_segment_stop), remote.rs fails the uploader on it (module doc, lines 36-40: "Only attaching the store anew takes the branch again"), Failure::errno maps it to EROFS (remote.rs:260, TASK-160.12), every commit after fails, and the VMM stops the guest; nothing in extentd attaches the store again. The case is the one fencing exists for, a takeover of a writer still running (doc-7 case 4, takeovers_while_uploading): the old placement has several segments uploading at once (IN_FLIGHT, 16) when the new one lists the branch and fences after the last it finds; any of the old one's that lands after that, at a number past the fence, takes the place of the new writer's next segment. So a failover from a host still alive (cut off from the control plane, not from the bucket) stops the guest on the new host at its first upload as well as on the old one. The simulation ends such a session (World: writers stopped are not ready) and checks nothing of the guest's availability. Failing test committed on review-storage, ignored with this task: fxdax remote::tests::takeovers_of_writers_still_uploading_keep_their_guests (cargo nextest run -p fxdax --run-ignored only -E 'test(takeovers_of_writers_still_uploading_keep_their_guests)': the commit gives Err(EROFS), the uploader Remote(Displaced)). TASK-165.9 moves the uploader into extent_store: fix with it or before it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A store whose writer is displaced by an earlier writer's segment landed past its fence takes the branch again (a new epoch, fenced past every segment landed) and its commits go on, none acknowledged lost, the ignored test passing un-ignored
- [x] #2 The simulation's takeovers_while_uploading checks that a displaced writer's host takes the branch again and its guest's durable commits reach the branch
- [x] #3 doc-7 and remote.rs say the same of Displaced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Taken into TASK-165.9 (the user, 2026-11-06). How its design resolves it: a WAL put after the branch's end by commits (Wal::rebase) is what opening a store does; a writer displaced (Error::Displaced, an earlier writer's segment landed past its fence where its next goes) is the same situation live. The uploader, extent_store's now, keeps the store and takes the branch again instead of failing: a new lease (Writer::open, a new epoch, fenced past every segment landed), the store's queue of unacknowledged segments rebased by commits (Store::rebase: those landed acknowledged, the rest stamped anew in memory, the open segment's chain moved on), and uploads go on; the guest's commits wait meanwhile, none acknowledged lost. The late segment is void (it follows no segment of the branch), so the branch's last commit is one of the store's own and the WAL follows; Diverged stays a failure (ESTALE) for a WAL another writer's commits moved past. Fenced (a later writer's segment, or the lease another's) stays fatal: that is losing the branch. remote.rs's module doc and doc-7 say the same. The simulation's takeovers_while_uploading checks a displaced host takes the branch again and its durable commits reach the branch. Lands right after TASK-165.9's design commit, on task-165-9.

Fixed in 3a63ada14 on task-165-9. extent_store::upload: an upload, gather or acknowledgement failing Displaced has the uploader take the branch again (Shared::after, Shared::retake): the branch's record and recovery, Writer::open_recovered with the same placement's token and a new holder, then Store::rebase (Queue::rebase: extent_remote::Wal::rebase over the queued footers, those landed acknowledged through Store::acknowledged, the rest restamped in memory and to be taken again; the open segment's chain set after them, Segments::set_chain), the new writer's lease, floor and progress (Shared::took), and the uploads go on, the guest's commits waiting meanwhile. Diverged, Fenced, Deleted still stop it. takeovers_of_writers_still_uploading_keep_their_guests un-ignored and rewritten for the new opening (the store's opening is the takeover): the commit succeeds, the lease epoch moved on by one, the branch holds it; with the retake off, EROFS and Displaced. The simulation's displaced sessions take their branch again with the same token, keeping their guests (World::retake), and opened() checks the log put after the branch ends with the guest's last commit (a violation otherwise; a log diverged ends the session, as the store refuses): reached in ten profiles, the 48 sim tests pass. doc-7 Fencing and upload.rs's module doc say so; OPEN_QUESTIONS.md has the decision line.

Verified: CI 42352682020 on land-9 (landed as c9207e5d4) green, Linux workspace included; macOS workspace 2079/2079 (scripts/test.sh at c9207e5d4); scripts/upstream-changes.py --check passes. AC #2: the simulation's world retakes on Displaced (sim/world.rs, takeovers_while_uploading holds); AC #3: doc-7 says a displaced writer takes the branch again, as extent_store::upload's docs do.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A writer displaced by an earlier writer's segment landed past its fence takes the branch again (Shared::retake: a new holder of the same placement, Store::rebase of the queued WAL), its guest's commits going on, none lost. Landed with TASK-165.9 (3a63ada14); the simulation's takeovers_while_uploading retakes too. Verified by CI 42352682020 and the macOS workspace.
<!-- SECTION:FINAL_SUMMARY:END -->
