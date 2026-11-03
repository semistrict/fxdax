---
id: TASK-150.6
title: Freed extent IDs can be minted again after a take
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 16:50'
updated_date: '2026-11-03 08:50'
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
- [x] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [x] #2 Fixed, the test red again with the fix taken out
- [x] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [x] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed, not fixed: extents_freed_are_spent_on_the_host_that_takes_their_branch: write Ok, expected EPERM, not committed as it would break the suite: branch A commits extent 1, frees it, commits, drops; create_for_branch elsewhere, attach_remote, write extent 1. Needs a format change: spent runs in remote checkpoints, replaying footers' frees on top. Refusing below the floor would break the journal's tail (images_opened_again_take_synced_writes). Blind spot: written-once tested on one host. A decision for the user.

Handed back 2026-11-02 (triage): still a bug on main: check_opens (crates/extent_store/src/store.rs ~812) consults only the local spent set, and adopt (~1175) carries none from the bucket. Needs TASK-153.9's format change, whose AC2 is this fixed; the confirming test extents_freed_are_spent_on_the_host_that_takes_their_branch was left out as it would fail the suite.

Fixed 2026-11-03, with TASK-158 (its WIP carried the fix). The branch's state in object storage (extent_remote::State) keeps the sequence numbers of the branch's own extents it held and freed (Spent, moved to extent_frame), replayed from footers' frees and kept in checkpoints; a store taking the branch (remote.rs Shared::take) spends them, so check_opens refuses them (EPERM). An extent freed while open, never sealed, as Fxfs's journal tail, it still writes. This is the format change the notes said it needed; nothing else of TASK-153.9 was required.

Test: extents_freed_are_spent_on_the_host_that_takes_their_branch (fxdax remote.rs), two ways the state comes, from a checkpoint (store closed) and from footers alone (host lost), plus the open-and-freed extent that must still write. Red with Shared::take's spend taken out (write Ok(()), expected Err(EPERM)), green with it, macOS 2026-11-03.

Blind spot: written-once was tested on one host, where the local index has Spent; nothing took a branch elsewhere and wrote a freed ID. Looked around: Shared::take is the one place a store gets a branch's state from the bucket; it adopts the extents, spends what was spent, commits the root, so nothing else of the local index that guards written-once is left behind. A point's copy (TASK-158) takes the same way, and writes nothing of the point's branch.

Structure: the record of what's spent now lives in the state that crosses hosts, as the extents do, rather than in the local index alone: one owner of it, which the bug came from not having.

Linux: GCE fxdax-test-t158-r1, cargo nextest run --workspace 2189/2189, this test included.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Freed extent IDs stay spent on a host that takes their branch: the branch's state in object storage carries Spent, and the taking store spends it. extents_freed_are_spent_on_the_host_that_takes_their_branch red without the spend, green with it; GCE r1 workspace 2189/2189.
<!-- SECTION:FINAL_SUMMARY:END -->
