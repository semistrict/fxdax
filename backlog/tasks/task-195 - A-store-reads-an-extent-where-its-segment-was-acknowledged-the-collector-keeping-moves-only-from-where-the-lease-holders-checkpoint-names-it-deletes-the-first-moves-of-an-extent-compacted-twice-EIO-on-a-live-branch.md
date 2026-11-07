---
id: TASK-195
title: >-
  A store reads an extent where its segment was acknowledged; the collector,
  keeping moves only from where the lease holder's checkpoint names it, deletes
  the first moves of an extent compacted twice: EIO on a live branch
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 06:45'
updated_date: '2026-11-06 13:04'
labels:
  - storage
  - collector
dependencies: []
priority: high
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the storage review of 2026-11-06. Since TASK-165.8 (a31f82a98) a store names an extent its uploader acknowledged at its segment's key, Data::Remote(segments/<branch>/<seq>) (extent_store::Store::acknowledged, state.rs acknowledge), and a store taking a branch (remote::take, a fork's child) at the locations the recovered state named then; nothing updates them afterwards: Writer::checkpoint follows the collector's moves in the writer's own state only (take_moves). The collector (TASK-175's rule ways_from_seals, collector.rs behind/Pass::new) keeps the moves on an extent's way from where the newest checkpoint names it when that checkpoint is the lease holder's, assuming every reader names it no further back. The store does: the fetcher (fetch.rs Fetcher::read, moved) goes from a gone object to moves/<key> and fails Lost if that is gone too. So: segment S compacted into M1 = moves/S; the writer checkpoints naming the extent in M1; M1 compacted into M2 = moves/M1; the next pass deletes S (named nowhere) and M1 (moved_long_ago); the store reads S, gone, M1, gone: Lost, the guest EIO on a committed extent the branch holds (a fresh recovery reads it through M1's moves). The host cache (Cache::adopt) hides it while it keeps S, until it evicts it; with no cache it shows at once. Views and followers and forks' children read the same locations. The simulation and specs/bucket model readers as the writer's state, not the store's persisted locations (the blind spot; TASK-88 is its kin). Failing test committed on review-storage, ignored with this task: fxdax remote::tests::extents_compacted_twice_read_where_the_store_names_them (run: cargo nextest run -p fxdax --run-ignored only -E 'test(extents_compacted_twice_read_where_the_store_names_them)', gives Err(EIO) where the data is expected). TASK-165.9 is changing this area: fix with it or before it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store reads every extent it holds after any number of compactions and collector passes, its writer checkpointing between them, with and without the host cache (the ignored test passes, un-ignored)
- [ ] #2 The simulation or specs/bucket models a host reader holding locations as the store holds them (where acknowledged or taken), and a mutation or rule-off run shows the fix needed
- [ ] #3 Forks' children taken from the bucket and followers are covered by the same test or its kin
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Taken into TASK-165.9 (the user, 2026-11-06). How its design resolves it: the uploader moves into extent_store and holds both the branch's writer and the store, so one owner keeps the store's locations with the writer's. Writer::checkpoint gains a hook where it takes the collector's moves (take_moves, before the checkpoint lands under moves_taken_first): the uploader has the store name every extent it holds there too (Store::moved: Data::Remote of held and kept extents, forward along their ways only, layout::further), so the store never names an extent further back than the lease holder's newest checkpoint, which is what the collector keeps moves from (collector.rs behind / Pass::new). Followers read the same store state (views), so they follow. Forks' children have no taken copy any more: a child's store opens by recovery from the bucket, which names its extents as the newest checkpoint does, and its own uploader keeps them so after. Points have no writer and no checkpoints of their own here: left as they are, noted. The simulation gets a host reader holding locations as a store does (set at acknowledgement, moved only by the writer's checkpoint hook), checked readable after each pass, with a rule (stores_follow_moves) whose mutation shows the fix needed. Lands right after TASK-165.9's design commit, on task-165-9.

Fixed in 3402efc60 on task-165-9, after TASK-165.9's design (872d739e7). Writer::following: a writer tells what follows the collector's moves with it of the extents its checkpoints record as moved, as take_moves names them (before the checkpoint lands, with moves_taken_first); the uploader has the store follow (Store::moved, State::moved: held extents' Data::Remote and kept ones', forward along their ways only, layout::further), from the store's opening and each retake. AC #1: extents_compacted_twice_read_where_the_store_names_them, un-ignored and rewritten for the new opening: one store open all along, its writer checkpointing as every CHECKPOINT_EVERY segments, the collector compacting the segment, the store freeing, checkpointing, the collector compacting the moves and deleting the segment and the first moves; the store and a follower of it read the extent, with the host's cache and without; with the hook off the store's read is EIO. AC #2: the simulation's hosts keep their stores' names (set as their writers open, extended at each acknowledgement, moved only by Writer::following), checked readable with the writers' own; rule stores_follow_moves (extent_remote Rules, simulation only: specs/bucket models the writer's state, not a store's names), sim::stores_need_to_follow_moves finds a store's name lost with it off (seed 2: 'branch b's store: lost ... in segments/.../1'); the 48 sim tests pass with it on. AC #3: a follower reads through its writer's store, in the test; a fork's child opens by the same upload::open and its own writer follows the same way: the test's fork, tried, kept the first moves alive for the child and so could not show the parent's case, and is not in it. Decision line in OPEN_QUESTIONS.md (Storage).

AC #3, then: forks_read_what_they_inherited_compacted_twice (a fork made, its parent deleted, its store open across the two compactions and its own writer's checkpoint) passes, EIO with the hook off; committed with the Shuttle test's wider expectation.
<!-- SECTION:NOTES:END -->
