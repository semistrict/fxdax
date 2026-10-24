---
id: TASK-42.3
title: >-
  Followers in extentd, doc-4 and Firecracker: read-only opens, ADVANCE, PIN,
  commit notifications
status: In Progress
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-24 00:53'
labels: []
dependencies:
  - TASK-42.2
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 123000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host side of a follower (doc-8, extentd and the device protocol): a second connection to a store gets a view instead of EBUSY, the writer's commits notify followers, and the device tells the guest. Loud failures (doc-8 testing 1) live here: extentd refuses any read of a read-only connection outside its view and pins.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 OPEN with a read-only flag gives a view of a store another connection holds, or of the store as of its last commit if none does; a writer may open it meanwhile
- [x] #2 On a read-only connection READ, MAP (read-only), ROOT and INFO are served from the view; WRITE, COMMIT, FREE, FORK, RESIZE fail with EROFS; ADVANCE, PIN and UNPIN work
- [x] #3 A read of anything outside the connection's view and pins fails with EIO and logs an error
- [x] #4 The writer's commits notify read-only connections, and the device notifies its guest; doc-4 specifies it, and the read-only feature bit
- [x] #5 extentd tests: writer and follower connections side by side, the writer never waiting
- [x] #6 Firecracker's extent configuration takes read_only, a commit in the fxdax-v1.16.1 series (the branch vendor/firecracker tracks)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: View::newer (waits for a commit after the view's, with a timeout: what notifies a follower), Store::view_extent (an extent's length as of a view, refusing what it doesn't hold), page-ins of kept extents (read_kept as the cache's source's fallback). 2. fxdax ExtentStore::follow: an ExtentStore of the writer's, sharing its store and the extent cache, with its own window and a view: reads and maps from the view, refusing anything outside it and its pins with EIO, logged (loud failures); writes, frees, commits, forks, resizes EROFS; advance (keeping the previous commit until released), release, pin, unpin, root and info from the view. 3. extentd: OPEN_READ_ONLY gives a follower of the store a writer connection holds, or, with none, of the store as of its last commit, opened as a writer would, which a writer's open then takes over; ADVANCE, RELEASE, PIN, UNPIN requests; a COMMITTED message to each read-only connection on the writer's commits; tests: writer and followers side by side, the writer never waiting. 4. RemoteExtentStore (client): read_only opens, advance/release/pin/unpin, commit notifications. 5. doc-4: the read-only feature bit (VIRTIO_EXTENT_F_READ_ONLY), ADVANCE/RELEASE/PIN/UNPIN, the committed epoch in the configuration space and its configuration change. 6. Firecracker: extent read_only configuration, the feature bit, the new requests, configuration changes on COMMITTED (in the vendor/firecracker fork, fxdax-v1.16.1 series and branch fxdax).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Steps 1-5 built (2026-10-18): extent_store views read only what their commits hold (Rules::views_hold_their_commit), advance_keeping/let_go, View::newer/Store::newer, view_extent, cut_views; ExtentStore::follow (reads/maps through the view, EIO logged outside it, EROFS for changes, ESTALE once cut off), take_over, cut_followers; extentd: OPEN_READ_ONLY, ADVANCE=21, LET_GO=22, PIN=23, UNPIN=24, COMMITTED=105 (a thread per read-only connection waits on Store::newer, 100 ms poll for going); one instance of a store per daemon for its writer and followers (stores::{followed, take_over, register, retire}); client connect_read_only/advance/let_go/pin/unpin/on_committed/committed; doc-4 Followers (F_READ_ONLY bit 5, ADVANCE/LET_GO/PIN/UNPIN 11-14, committed config field, ESTALE); doc-8 updated. Decision (made unattended): a writer opening a store its followers have open takes their instance over if nothing is open in it (followers opened it, or its writer went after its last commit), and its followers go on following; a writer that went with writes uncommitted retires its instance: followers cut off (ESTALE), and writers and followers get EBUSY until they are gone (a fresh open then starts from the last commit). Why: reusing an instance with uncommitted writes would hand the next writer extents its filesystem doesn't know of. The daemon's control work on a followed store: sync, grow and fork go through the instance; delete, place, make_public are EBUSY. Tests: extentd (Linux, GCE): followers_follow_their_writer_side_by_side, writers_take_over_stores_their_followers_opened, writers_that_go_with_writes_uncommitted_cut_their_followers_off, stores_followed_are_worked_on_as_they_are; extents: writers_take_over_stores_followed and the follower tests; views: views_cut_off_by_the_store_fail. Step 6 (Firecracker) in progress in the worktree's vendor/firecracker: F_READ_ONLY, requests 11-14, committed with config changes, read_only config, a reset follower reopens its store at activation.

Verified (GCE x86-64, 2026-10-18): extentd tests followers_follow_their_writer_side_by_side (AC1, AC2: reads/root/info from the view, EROFS for write/free/commit/fork, ADVANCE/LET_GO/PIN/UNPIN; AC3: EIO outside the view, which ExtentStore::outside_view logs; AC6: the writer commits 17 times while followers lag and pin), writers_take_over_stores_their_followers_opened (AC1, no writer), writers_that_go_with_writes_uncommitted_cut_their_followers_off, stores_followed_are_worked_on_as_they_are (RESIZE: a follower's set_capacity is EROFS, extents test followers_change_nothing). Firecracker (vendor/firecracker, local branch fxdax-followers, b90092950 + 1b3d5edd3, unpushed): vmm extent tests 7/7 incl. test_follows_a_writer, firecracker API extent test 1/1; b90092950 fixes test_roots_and_info, stale since the bucket became mandatory (sequence_floor 1<<40). AC4's device side (configuration change to the guest) waits for the guest driver (42.6) to show it end to end; AC5 waits for the fork's push (needs the user's OK).

Refined by 7a36a35 (found by the first guest follower, TASK-42.6): AC3's loud EIO is for reads of extents the view let go of (freed before the commit it holds since, unpinned: NotHeld); an extent written after the view's commit reads as ENOENT, as Fxfs's journal replay reads on to the first extent its commit doesn't hold. doc-4 updated.

2026-10-23 audit: AC5 reworded: the submodule tracks fxdax-v1.16.1, where a7433cfd6 (read_only, vmm_config extent) is and is pushed; the branch fxdax stopped at cf70a721b on 10-17 and is no longer used. AC4's evidence: doc-4's F_READ_ONLY and committed, extentd's COMMITTED (followers_follow_their_writer_side_by_side, extentd/tests.rs), fxfs-guest-follow 10/10 (TASK-42.10's notes).
<!-- SECTION:NOTES:END -->
