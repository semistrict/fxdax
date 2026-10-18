---
id: TASK-45.1
title: Fence branch deletion with the placement token
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 09:22'
updated_date: '2026-10-18 09:45'
labels: []
dependencies: []
parent_task_id: TASK-45
priority: high
type: bug
ordinal: 140000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_remote::branch::delete writes a final lease (token u64::MAX) without comparing its token with the lease's, so a host whose placement is stale (cut off, the VM moved on) can delete a branch a newer placement is writing; its writer then stops at its next acknowledgement and the branch is lost. fxdax::remote::delete_branch passes no token at all.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A delete with a token lower than the branch lease's token fails (Stale) and leaves the branch as it was
- [x] #2 ExtentStore::delete, extentd's DeleteStore and fxdaxctl delete pass the store copy's placement token
- [x] #3 The simulation shows a stale placement's delete refused, and the rule shown needed (turned off, the simulation fails)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
extent_remote::branch::delete takes the deleter's token and fails Stale when the lease's is higher (rule delete_tokens); fxdax passes the store copy's placed token, or the host's; ESTALE through ExtentStore::delete and DeleteStore. Simulation: stale_placements gains stale deletes and honest deletes racing later placements; deletes_need_tokens finds a violation at seed 0 with the rule off. Red check: earlier_placements_delete_nothing fails when fxdax passes u64::MAX. GCE 2066 + macOS 2024 green.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Branch deletion is fenced: a placement whose token is lower than the branch lease's gets Stale (ESTALE through fxdax), the branch untouched. Shown by the simulation (rule needed) and remote::tests::earlier_placements_delete_nothing.
<!-- SECTION:FINAL_SUMMARY:END -->
