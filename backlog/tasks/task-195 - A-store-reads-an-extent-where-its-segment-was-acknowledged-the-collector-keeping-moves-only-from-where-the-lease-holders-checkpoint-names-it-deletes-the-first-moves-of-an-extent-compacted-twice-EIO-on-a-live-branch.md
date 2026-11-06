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
updated_date: '2026-11-06 07:07'
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
<!-- SECTION:NOTES:END -->
