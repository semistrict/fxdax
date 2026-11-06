---
id: TASK-149
title: Testing ranked by how likely it is to find bugs
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
updated_date: '2026-11-06 02:22'
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

2026-11-01, found running the fxdax crate six times under load, with subagents beside it. SIGTERM killed extentd as it started: on_sigquit's thread blocked signals itself, on its first run, and it starts before main blocks SIGTERM, so a signal arriving before it ran went to it and took the default action. 216 of 600 runs of the_daemon_stops_on_sigterm under 40 CPU hogs died of it; 0 of 600 after spawn_taking_no_signals blocks them before the thread starts. Blind spot: a race needing a thread not yet scheduled, which an unloaded machine hides; it read as a one-off flake. Test: a_thread_taking_no_signals_takes_none_from_its_first_instruction asserts the rule from the thread's first instruction, red with the block removed. Looked around: pthread_sigmask's other users in crates and test_cases: extentd's SIGTERM thread inherits its block, so none like it. hundreds_of_writers_flush_on_threads_they_share: not a bug: extentd io threads follow concurrent guests (about 8 for 3, 18-25 for 9, 58-70 for 33), flat over 150, 300 and 600 stores; the bound of 64 was tight. Now the runtime's threads must be under a quarter of the stores: 6 of 6 full-crate runs passed, and 4 of 4 failed at 277-318 threads with flushes put back on a blocking thread each. Harness, r393: script_run's tests assumed FICLONE on the host's temporary directory, which GCE's ext4 refuses, and an expected SIGBUS dumped core, setting the status's core bit, 777 cores. Blind spot: tests written in Docker never ran on a GCE host. They now use an XFS loop with reflinks, and the child is not dumpable. 9 of 9 script_run tests pass with core dumps on.

r394 (7082ef35d plus the SIGTERM, script and bound fixes; xfstests passing on both, 14 and 16 known failures) failed scripts/dax-refusals.py: one unexplained refusal of 55 in xfstests-fxfs-guest-dax: a store at window offset 0x20b85000 to extent 1099511631341 'read-only; open in the window; in the store: 0x2f000 bytes, open', mapped read-only 0x50000 bytes from 0x20b36000 1.2 ms before, the next page refused as nothing mapped at once after. TASK-99's signature: an open extent mapped read-only that the guest stored to. The guest went on; TASK-99's stopped the VM. r389 had none, and nothing in dax.c changed between them, so a rare race, not yet diagnosed. Results kept in /tmp/fxdax-r394-xfstests.

2026-11-05: TASK-149.29 done (fxfs-sim-diff in CI 42137709782's workspace job; it found a model drift and a real Fxfs bug, both fixed). AC #1 open: 17 subtasks are To Do (149.2, 4, 10 to 20, 22, 24, 25, 27).
<!-- SECTION:NOTES:END -->
