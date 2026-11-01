---
id: TASK-150.1
title: Collector deletes objects of inherited extents still uploading
status: In Progress
assignee: []
created_date: '2026-11-01 16:49'
updated_date: '2026-11-01 19:11'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 516000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_remote/src/collector.rs:421-427, key at crates/extent_frame/src/segment.rs:218: the in-flight check on an extents/ object uses the branch in its key, which minted the extent, not the branch uploading it. A child with a branch of its own uploads inherited extents of 4 MiB or more as own (remote.rs diff), a pass lists the object before its segment lands, and deletes it after the segment is acknowledged. Reads then fail with Error::Lost once the local copy is gone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: what_a_child_uploads_of_what_it_inherited_outlives_the_collector failed with Error::Lost. Fixed: inherited extents go in segments; Contents and Commit name own extents by sequence number, so can't name another branch's; Chain::admit refuses a footer that does. Blind spot: the sim's writers seal only IDs they minted, and the fork sweep ran passes only between steps. Class test: Mishap::Collected, a pass before each write; each_write_of_a_childs_upload_meeting_the_collector went red with the bug back. Around it: only the collector reads Kind::Extent's branch. Objects landed under old keys could leak once freed: bounded, untested. Structure: the collector infers what is in flight from object names (TASK-79.3, 79.18, 79.73, this): raised with the user. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.
<!-- SECTION:NOTES:END -->
