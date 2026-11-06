---
id: TASK-204
title: >-
  A store taking a branch over from a writer still uploading fails for good
  (Displaced, EROFS) when that writer's segment lands past its fence: doc-7 says
  it takes the branch again, keeping its guest
status: To Do
assignee: []
created_date: '2026-11-06 06:55'
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
- [ ] #1 A store whose writer is displaced by an earlier writer's segment landed past its fence takes the branch again (a new epoch, fenced past every segment landed) and its commits go on, none acknowledged lost, the ignored test passing un-ignored
- [ ] #2 The simulation's takeovers_while_uploading checks that a displaced writer's host takes the branch again and its guest's durable commits reach the branch
- [ ] #3 doc-7 and remote.rs say the same of Displaced
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
