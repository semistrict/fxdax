---
id: TASK-150.6
title: Freed extent IDs can be minted again after a take
status: To Do
assignee: []
created_date: '2026-11-01 16:50'
updated_date: '2026-11-03 02:10'
labels: []
dependencies:
  - TASK-153.9
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 521000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/extent_store/src/store.rs:1171 (adopt), :810-820 (check_opens): Spent lives in the local index, so a taken store starts with it empty, and sequence_floor is advisory. A guest can write an ID it sealed and freed, breaking written-once IDs, and a fork still holding the old extent shares its cache entry.
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
Confirmed, not fixed: extents_freed_are_spent_on_the_host_that_takes_their_branch: write Ok, expected EPERM, not committed as it would break the suite: branch A commits extent 1, frees it, commits, drops; create_for_branch elsewhere, attach_remote, write extent 1. Needs a format change: spent runs in remote checkpoints, replaying footers' frees on top. Refusing below the floor would break the journal's tail (images_opened_again_take_synced_writes). Blind spot: written-once tested on one host. A decision for the user.

Handed back 2026-11-02 (triage): still a bug on main: check_opens (crates/extent_store/src/store.rs ~812) consults only the local spent set, and adopt (~1175) carries none from the bucket. Needs TASK-153.9's format change, whose AC2 is this fixed; the confirming test extents_freed_are_spent_on_the_host_that_takes_their_branch was left out as it would fail the suite.
<!-- SECTION:NOTES:END -->
