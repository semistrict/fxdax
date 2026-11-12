---
id: TASK-391
title: >-
  sim::recoveries_need_newest_checkpoints takes 546 s on main, 19 s before: its
  first violation now at seed 4985 of recreated()
status: To Do
assignee: []
created_date: '2026-11-12 22:35'
labels:
  - testing
  - bug
dependencies: []
priority: medium
ordinal: 768000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On main (76df8f0f3), with no change of mine to the simulation, the rule-off search for newest_checkpoint runs 546 s on an M-series Mac before seed 4985 finds 'branch 2's log diverged, though the branch's last commit, None, is the log's'. On 2026-11-11 (TASK-293's run, before the 42 commits since, among them TASK-371's takeovers) it passed in 18.7 s. takeovers_need_takeovers_kept, deletions_need_fences and collection_needs_candidates_first ran past 430 s in the same run. The simulation's change moved the seeds that make these cases; a profile likelier to make each is needed, as needed()'s doc says.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 recoveries_need_newest_checkpoints finds its seed in under 60 s
- [ ] #2 The other test_needs! searches past 120 s each get a profile likelier to make their case, or a task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
