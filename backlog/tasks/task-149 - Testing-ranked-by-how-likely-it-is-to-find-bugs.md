---
id: TASK-149
title: Testing ranked by how likely it is to find bugs
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 08:13'
labels: []
dependencies: []
priority: high
ordinal: 501000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On 2026-10-31 a model test of fxfs-posix with power cuts, a tiny device and fragments found seven real Fxfs bugs in a day, one able to leave a volume unmountable and one a super-block recording progress the journal hadn't flushed, after xfstests and the guest suites had passed for weeks. What worked was random operations, a precise oracle, failures combined, and shrinking to a replayable case. Yet about 72% of the last 400 commits' fixes (167 of 231, by keywords) were in DAX, the VMM and the guest kernel, and 10% in Fxfs core, and the open bugs (TASK-137, 79.82, 79.78) are in the VMM and in forks. So this ranks work by how likely it is to find bugs, not by cost: its subtasks are numbered in that order, with existing tasks placed among them by ordinal: 149.1, TASK-44.15 and 149.2, TASK-44.10, 149.3, 149.4, then the rest. A rank is a judgment, not a measurement: each task's notes should record what it found, so that the ranking can be corrected.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each subtask's notes say what it found, or that it found nothing
- [ ] #2 The ranking is corrected when a result contradicts it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Applying AGENTS.md's blind spot rule to the guest failures of r380 and r381. generic/303 panicked a guest (22 MiB journal write) because a clone's byte limit counted holes. Blind spot: the model test clones only near small offsets, and no test clones across a huge hole. Class: a limit that counts range rather than work. Added test_clone_range_crosses_holes_at_once. Still to do: Clone in model_tests.rs at awkward offsets.

Results so far against the ranking. TASK-44.10's fork sweep found 7 bugs, all fixed, on test/fork-faults (194f937d7), among them data loss by the collector and a failure of every collector pass for every tenant: the best yield yet, ranked third. TASK-149.1 (rust-checks) found none new, ranked first: too high. The model test found 8+ in Fxfs core. GCE guests found 3, all in this week's fixes, none of which any host test could reach. Rank fault sweeps of host paths above assertion builds.

The graveyard reap that lost a store (r380): blind spot: a window too narrow for the single-threaded model, so a journal that compacts all the time, added to the model, did NOT catch the bug put back (1500 cases, 3738 cuts passed). Closed instead by an invariant: initial_reap of the root store asserts compactions are paused. Lesson: to close a narrow race, assert the rule, as a sampling test may pass. The small journal stays, for what else it may find.
<!-- SECTION:NOTES:END -->
