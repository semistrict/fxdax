---
id: TASK-150.7
title: A guest or VM process can wedge its own commits
status: Done
assignee: []
created_date: '2026-11-01 16:51'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: low
type: bug
ordinal: 522000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extents.rs:1081-1089: the open-extent path does not check alignment, so a guest WRITE leaves open.len unaligned, and every later commit fails EINVAL. crates/fxdax/src/extentd/server.rs:1041: WRITE, MAP writable, FORK without FORK_QUIESCENT seals the extent while open holds it, and later commits fail EPERM. Harm is limited to the caller store.
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
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: writes_to_extents_mapped_writably_are_of_whole_blocks, forks_seal_nothing_a_window_maps_writably (EINVAL, EPERM on later commits). Fixed: ExtentStore::write has one check, check_io, whichever way it goes; fork gives EBUSY rather than seal an extent held both ways. Blind spot: serves_whatever_a_guest_asks had no oracle that commits work, and its writes and maps almost never met. Class test: it, red for both. Structure: open has two owners (TASK-79.46, 121, this): raised with the user. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Both halves confirmed and fixed (206d0a1a3, 5586407ac, 1ef23d71b; d559e9e78 made the mapped extent's memory its one owner): writes always pass the one check_io, and a fork returns EBUSY rather than seal an extent written and mapped writably (writes_to_extents_mapped_writably_are_of_whole_blocks, forks_seal_nothing_a_window_maps_writably; serves_whatever_a_guest_asks checks commits keep working, red for both).
<!-- SECTION:FINAL_SUMMARY:END -->
