---
id: TASK-150.19
title: 'On a truly full filesystem, truncated bytes can come back'
status: Done
assignee: []
created_date: '2026-11-01 21:43'
updated_date: '2026-11-03 10:30'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 542000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-149.8's hunts, confirmed by the model test: zero_tail gets NoSpace on a cut because the cut's frees wait for a sync; a sync and retry, now in setattr, narrows it, but with no block free even after a sync the bytes past the new end stay, and a later grow exposes them. Failed 2 of 2,880 200-op sequences and 0 of 16,800 at 48 ops, so the default model test could very rarely go red. Not pinned, as a red test can't land; the replay sequence is in TASK-149.8's notes. Two ways out, a decision for the user: every non-DAX growth through one path that zeroes the tail in its own transaction, dropping the eager zero, which is the proper fix (TASK-80's reserved writeback would pay for that block, and it touches Fxfs, the page cache and writeback accounting); or a mid-block shrink returns NoSpace when no block exists, reversing TASK-60 for that case. Same owner of size question as TASK-150.15 and TASK-133, and explains generic/130 and TASK-79.53.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The user has chosen how to close it
- [x] #2 A test fails on the code as it stands, pinned once it can land
- [x] #3 Fixed, the test red again with the fix taken out
- [x] #4 The notes name the blind spot and what a search around it found
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03: fixed by TASK-153.4 (5afb21c77). The choice, every non-DAX growth zeroing the tail through one path, is recorded under # Decisions in OPEN_QUESTIONS.md, as the never-ask rule has it, for the user to overturn. TASK-149.8's replay now lands as a_truncate_with_no_block_to_zero_its_tail_in_leaves_nothing_past_the_end: red before the fix (differing at 54584, the truncated size, after a clone grew the file), green after, red again with txn_grow_size's zeroing taken out. Blind spot: the eager zero at the cut stood for what each growth should have done, so no test grew a file past a tail a cut had left unzeroed: every_growth_finds_what_a_cut_left_past_the_end_zeroed now does, by each way a file grows. Search around it: writes past the end, allocations and clones all grew without zeroing; a guest clone grew the inode without zeroing its cached last page (store_past_end_then_cloned).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Truncated bytes no longer come back after a truncate with no room to zero its tail: every growth zeroes it instead (TASK-153.4, 5afb21c77). The replay is pinned, red before and with the fix out. Verified with TASK-153.4's runs: macOS, and on GCE the Linux workspace, guest checks, script oracle and xfstests, DAX and not.
<!-- SECTION:FINAL_SUMMARY:END -->
