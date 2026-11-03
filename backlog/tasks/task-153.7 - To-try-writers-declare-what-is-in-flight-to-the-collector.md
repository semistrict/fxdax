---
id: TASK-153.7
title: 'To try: writers declare what is in flight to the collector'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 13:39'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 551000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The collector infers in-flight objects from names: TASK-79.3, 79.18, 79.73, 150.1. Format change: do with the others.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decided, with the format changes together
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (2026-11-03), with TASK-153.8, 153.9 and 141.2 in one format change.

What the collector infers: whether an object no live state names may yet be named. A segment by its number (at or past its live branch's end). An extent stored on its own by its key, extents/<branch>/<epoch>.<seq>.<sequence>: the writer of <epoch> uploads it before its segment <seq>, which names it. That key is the declaration this task asks for: written by the writer, atomically with the object, saying which segment of which writer will name it. Its four bugs came from the declaration being incomplete (79.3: no <seq>; 150.1: <branch> the minting branch, not the uploader's), not from its being a name.

Decided: keep the declaration in the key, and make it exact. A separate declaration (an object or a lease field saying what a writer has in flight) was considered and rejected: it must land before the objects it covers and the collector must read it after listing them, two more orderings of the kind 79.3 and 79.18 broke; it is one more write per upload and an object kind whose own garbage needs collecting; and it would say nothing the key doesn't. (dead-ends/collector-in-flight-declarations.md.)

1. Format: an own extent's key always names its segment (drop keys of before TASK-79.3, Kind::Extent's seq: Option); Chain::admit already refuses another branch's extent on its own (150.1).
2. Collector: one function says whether an object may yet land, for segments and extents alike (Pass::may_land), from the branch's end and epoch: no other place reads keys' epochs and numbers.
3. Rule extent_keys_name_segments (off: the collector goes by the branch's next sequence number, as before TASK-79.3), shown needed by the simulation if a profile reaches it, else by the collector's unit test red with it off.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented, as planned. Kind::Extent's seq is no longer optional (keys of before TASK-79.3 read as Unknown); Pass::may_land decides for segments and extents alike from the branch's end and epoch, the one place keys' epochs and numbers are read; orphan() is gone. Rule extent_keys_name_segments: off, the collector goes by the branch's next sequence number; extents_ahead_of_their_segments_need_keys_naming_them reads Lost with it off, the extent gone ahead of its segment (the simulation's guests commit extents in their minted order, so no profile reaches it: a unit test shows it). dead-ends/collector-in-flight-declarations.md records why a separate declaration is not worth it. OPEN_QUESTIONS.md # Decisions.

Verified: macOS scripts/test.sh at e8e186485: workspace 2051/2051, Shuttle 61, 7, 6, 21, 10, 18, scripts 135 OK; the extent_remote simulation 37/37. GCE fxdax-test-t1537-r1 (us-east4-a, n2-standard-4): Linux workspace 2215/2215 and the same Shuttle suites; its guest stage stopped at a test_cases host test (XFS loop umount busy, no fxdax code), so r2 ran the guest cases alone: fxfs-guest-dax, -live-fork, -migrate-dax, -migrate-tenant-dax, -snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -suspend-dax all OK, test_cases 53/53. Both hosts deleted. scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Decided: a writer's declaration of what is in flight is the key of what it uploads, which names its epoch and the segment that will name it, written atomically with the object; a separate declaration would add two orderings and an object to collect, saying nothing more (dead-ends/collector-in-flight-declarations.md). Made exact in the format change: keys always name their segment, one collector function (Pass::may_land) reads them; rule extent_keys_name_segments shown needed by extents_ahead_of_their_segments_need_keys_naming_them. Verified macOS and GCE (workspace and 9 guest cases).
<!-- SECTION:FINAL_SUMMARY:END -->
