---
id: TASK-92.55
title: >-
  Opening a fork of a large base costs ~0.5 s of zstd on the remote thread,
  growing with the base
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-24 03:39'
updated_date: '2026-11-08 05:44'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 370000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b (TASK-92.17's test counting_followers_walks_no_extents). 100 forks of a base of 32,768 extents opened through the daemon on a GCE host (debug build) took ~55 s, about 0.5 s a fork (2 s before unpacked parts were zeroed at once). perf puts it on the 'fxdax remote' thread in ZSTD_compress2 / ZSTD_decompressSequences as each fork opens, so a fork's opening compresses or reads data that grows with its base, against TASK-34's rule that a connection's cost does not grow with its base.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Opening a fork costs the same at any base size, shown by a test counting what an open compresses and reads
- [ ] #2 100 forks of a 32,768-extent base open in a time flat in the base, on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Target (the user, 2026-11-08): a fork's opening at a base of 32,768 extents within 2x of one at 1,024 (debug, the test forks_open_in_a_time_flat_in_their_base: 100 forks, each at a fork point of its own, as r1021). Slices, each measured; stop once there.
1. A branch's extents in a persistent map (imbl::OrdMap, B-tree, std-like API; rpds's red-black tree iterates slower): a State copies for free between recovery, the writer, its checkpoints and the store's opening. Measured (Linux in Docker, debug, 2 runs each): 32,768 extents 44-49 ms a fork against main's 53-55; 1,024: 5-7 ms on both. ~15%.
2. Checkpoints decoded once a host: a host-wide cache of the states checkpoints hold, by key (create-only objects), each an Arc<State> on the persistent map, bounded by count; recoveries of forks starting from one share it (Checkpoints, today per recovery with keep 0).
3. No checkpoint for each fork (no format change; replaces the delta checkpoints proposed): the writer acknowledging a segment with forks makes them without a checkpoint at it: the child's record is made on its acknowledgement (as now), and the child's recovery starts from the parent's newest checkpoint up to the fork point, shared (2), with the parent's footers after it applied (at most CHECKPOINT_EVERY = 64), copying nothing of the base (1). The collector already keeps what recoveries read: checkpoints, the segments they replay (doc-7, Garbage collection 4), and the moves from where a replay from an older checkpoint names extents (TASK-175). What the per-fork checkpoint also did, moves followed and forks of deleted children dropped, the next periodic checkpoint does. doc-7 Checkpoints and Forks updated. Slice 1's profile at 32,768: the parent's checkpoint at each fork 49% of the open (following moves 16%, encoding 17%), the child decoding it 26%, the child's store open 11.5%.
4. The child's store opens in its own extents' time: what it inherited held as the bucket's, layered, not rebuilt into the store's map (extent_store Recovering::open walks every extent, summing sizes).
Tests: forks_open_in_a_time_flat_in_their_base (the measurement: unpacks, reads equal; 32,768 within 2x of 1,024); extent_remote sim (every rule test) and its mutants (crates/extent_remote/mutants.sh) for 3; GCE r1021 (kept) for the final numbers.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 sweep, at 1a3391d7c: the zstd is gone (TASK-165.x): a fork's opening unpacks no extent and reads no stored byte (stats().unpacks, read_stored_bytes: 0 at either base size), but its time still grows with the base. Measured with a test opening forks of a base of 1,024 and of 32,768 extents through the daemon (each extent one BLOCK, committed 1024 at a time): locally (Linux in Docker, debug) ~6 ms against ~56 ms a fork (20 forks); on GCE r1021 (n2-standard-4, debug, 100 forks) 6.0 ms against 124.5 ms, the 'fxdax remote' thread at 100% CPU for the whole 12.6 s. Profile (perf, local with full call chains; GCE leaves agree): per fork, the base's uploader, acknowledging the segment with the fork, writes a checkpoint of the base's whole state (upload.rs acknowledged -> writer.checkpoint -> extent_remote::checkpoint::encode, cloning the BTreeMap<ExtentId, Location> and encoding its table), and the child's opening recovers from that checkpoint (start -> extent_remote::recover -> recovery::checkpoint -> checkpoint::decode, building the same map; ~4% zstd of the checkpoint object). Both O(base extents). User's decision 2026-11-08 (OPEN_QUESTIONS.md b0d7d00d2): do it: checkpoints that point to the parent's instead of copying it, decoded state shared between forks.

2026-11-08: slices 1-2 on land-w10 (0f23a2ce9, CI 42451897096): a persistent extents map (imbl OrdMap) and checkpoints decoded once a host (Bucket::decoded). Fork opening, 100 forks, Linux in Docker: debug 32,768 extents 42-43 ms against main's 53-55, 1,024 ~5 ms; release ~11 ms against ~3.5 ms on main and slices 1-2 alike (~0.3 us an extent). extent_remote + extent_store 395/395 pass; fxdax's fork, remote and extentd fork tests 64/64. The user's decision (2026-11-08): stop at slices 1-2, the 2x debug target not worth a format change; the measurement test keeps unpacks and reads equal and says the times; delta checkpoints filed as TASK-242 (Low).
<!-- SECTION:NOTES:END -->
