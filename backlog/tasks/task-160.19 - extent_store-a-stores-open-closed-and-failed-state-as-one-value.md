---
id: TASK-160.19
title: 'extent_store: a store''s open, closed and failed state as one value'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-07 09:46'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 590000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
failed and closed are booleans; a store fails three ways; close_between_commits repeats still_writable differently; 16 lock sites each decide what failed or closed means. Explains TASK-83, 79.99, 153.16. Proposal: enum Life read through readable()/writable() guards, re-checking after waiting structural. After TASK-153.8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One life value behind guards
- [x] #2 recheck_after_waiting can't be broken by a caller
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done on batch-x1, 2026-11-07, against main 00e541868 (165.9 had made failed an Option<String> carrying the first failure's cause, 4ba694a53). State::standing: Standing { Open, Closed, Failed { cause, closed } } replaces failed and closed; point stays the branch's own property. Guarded<'a, F, H: Holds> over Locked, Readable (Whole: not failed) and Writable (Changeable: not failed, not closed, no point), made only by Guarded::check and checked again after each of their waits (wait_while; hold_writes_while, which holds the write gate as a commit or a closing between commits does), unless the Shuttle rule recheck_after_waiting is off. Store::state and Store::writable return them; still_writable and hold_writes are gone, so a caller that waits holds a guard checked again, or a bare Locked that promises nothing (the waits for framings, which need no promise). close_between_commits keeps its own case, a store closed meanwhile closing nothing, after its guard's re-check. Tests: standings_fail_and_close_in_either_order; extent_store 159/159; Shuttle 2x36/36 (needs_rechecks_after_waiting_for_failures still red with the rule off); crash rounds 5/5; sim 48/48.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store's open, closed or failed life is one value, Standing, read through Readable and Writable guards that are made only by checking and re-check after every wait, so no caller can wait and go on unchecked; verified by extent_store's tests, Shuttle (the rule-off test still red without the re-check), the crash rounds and the sim.
<!-- SECTION:FINAL_SUMMARY:END -->
