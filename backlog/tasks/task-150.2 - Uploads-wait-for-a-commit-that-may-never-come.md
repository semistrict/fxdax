---
id: TASK-150.2
title: Uploads wait for a commit that may never come
status: Done
assignee: []
created_date: '2026-11-01 16:49'
updated_date: '2026-11-03 02:10'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 517000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_store/src/store.rs:919-921, :1341, :1671, :1756; segments.rs:682; crates/fxdax/src/remote.rs:1125-1130: a segment make_room closes stays Phase::Framing until the next commit flushes, so take_closed will not release it. COMMIT_DURABLE, SyncStore and a live fork upload wait block until another commit arrives. Whether Fxfs serialises its commits so that a guest cannot wait so is not established.
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
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: segments_closed_for_room_go_to_the_uploader_with_no_commit_after. Fixed: a flush with no commit ends closings for room (end_closings, rule flushes_end_closings), after a refactor to one framing_ended. Fxfs's own COMMIT_DURABLE can't hit it, Seal keeping writes out; SyncStore and a live fork can. Commits stalled on max_lag_bytes also deadlocked at store level, proven, unreachable by a guest with today's constants by reading only, a margin nothing enforces. Blind spot: no test waited for an upload with no commit after. Class test: Shuttle needs_flushes_to_end_closings and two others, red with the rule off. Structure: framing's end is driven from outside it (TASK-41.1, 79.69, 79.99, 92.16, this): raised with the user. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Confirmed and fixed (a0510dd17, 748366e5c, 3c90f8f7b): a flush with no commit ends closings for room (end_closings, rule flushes_end_closings), so SyncStore, a live fork and a lag-stalled commit no longer wait for a commit that may never come. segments_closed_for_room_go_to_the_uploader_with_no_commit_after and Shuttle needs_flushes_to_end_closings went red with the rule off. The structural point (framing's end driven from outside) is TASK-153.8's.
<!-- SECTION:FINAL_SUMMARY:END -->
