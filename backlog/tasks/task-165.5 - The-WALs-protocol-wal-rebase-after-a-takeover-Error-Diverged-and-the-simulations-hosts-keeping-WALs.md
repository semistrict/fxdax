---
id: TASK-165.5
title: >-
  The WAL's protocol: wal::rebase after a takeover, Error::Diverged, and the
  simulation's hosts keeping WALs
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 16:10'
labels: []
dependencies:
  - TASK-165.2
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 627000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 5. A host's closed segments not yet acknowledged replace remote.rs's marker as the record of what may land (TASK-79.20, 79.72): after a takeover they are put after the branch's end by footers alone. The simulation's hosts re-batch their own commits today (World::opened), so the host side of the seam is not what it checks; with the production rebase it is.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote::wal::rebase (Wal::rebase) takes what the writer found at the branch's end, recovered and fenced, and a WAL's footers and base: by commits (doc-14 as TASK-165.2 changed it), drops the segments to the branch's last commit after the fence if it is one of the WAL's, restamps the rest on the writer's chain (own extents at the new keys) if it is one of the WAL's or its base, and otherwise fails with Error::Diverged
- [ ] #2 Unit tests: WAL wholly landed, partly landed, landed past a gap (void), none landed, another writer moved on, empty WAL
- [ ] #3 The simulation's hosts keep WALs of stamped segments, lost or kept at a crash, and every open on a host with one runs wal::rebase; all profiles pass
- [ ] #4 Rules gains wal_follows and wal_after_fence, each turned off by a test_needs! that finds a violation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Refactor: batch::Data, a closed segment's data framed with whichever footer stamps it.
2. extent_remote::wal: Wal<T> (base, segments with the host's data), push, acknowledge, head, rebase by commits (doc-14 as TASK-165.2 changed it); Fenced, what a writer found at the end (recovered and fenced last commits, chain), Writer::fenced; Error::Diverged (fxdax: ESTALE); Rules wal_follows and wal_after_fence with their TLA+ lines.
3. Unit tests over a memory bucket with real writers: wholly landed, landed under a later checkpoint, partly landed, past a gap, none landed (own extents at new keys), another writer moved on (Diverged), only a fence after, empty, a fork's segment alone, acknowledgement.
4. Simulation: hosts keep Wal<Data> (kept or lost at a crash), every open runs Wal::rebase (World::opened's re-batching goes), the open batch goes into the log as a session ends; a history oracle (each commit applied after its parent since the newest checkpoint); profiles wals (hosts restarting at once) and forked_wals; test_needs for both rules.
5. Verify: rule tests and SIM_SOAK=100 of the holds tests, check-tla (bucket), macOS workspace+shuttle, Linux extent_remote/extent_store/fxdax in Docker.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (2026-11-05). extent_remote::wal::Wal<T>: a host's log of a branch, its base (the branch's last commit before its first segment) and its segments by footer with what the host keeps of each (T: the sim's batch::Data; extent_store's files in TASK-165.9). Wal::rebase(&Fenced): the guest's last commit through each of the first n segments, n = 0 (the base) to all, a segment of no commit (a fork's alone) carrying the one before; the last n whose head is the branch's last commit after the fence (wal_after_fence; without it, as recovery found it) is how far it landed: those segments taken out and returned (acknowledged), the rest restamped on the writer's chain (Chain::close of their contents: new seq, epoch, after, next_sequence floor; own extents at the writer's keys by Footer::own). No n matches: Error::Diverged, the log unchanged (wal_follows; without it, all restamped). An empty log never diverges. The base is then the branch's last commit after the fence, whichever commit decided. Fenced { recovered, last_commit, chain } is captured in Writer::take before and after the fence loop; Writer::fenced() returns it, so the rule-off path needs nothing else of the writer. Error::Diverged maps to ESTALE in fxdax::remote::Failure::errno (nothing returns it there before TASK-165.9).
Refactor first: batch::Unframed became { footer, data: Data }, Data::frame(&footer) framing with any footer (the sim restamps through it).
Simulation: Host::disk (commit tags) became wals: HashMap<u64, Wal<Data>>; close_batch pushes each segment the writer took; Acked drops what it acknowledged (only for the current session; else the next rebase finds them landed); World::stop closes a session's open batch on its writer's chain into the log (the open segment, doc-14 step 3) at end, crash and quiesce; World::opened runs Wal::rebase (landed commits join acked; restamped segments are taken and uploaded, their forks' children's origins move to the new seq; a diverged log is dropped, as fxdaxctl take would, the spec's choice). Profile::restarts: a host crashing with its disk kept takes its branches back at once (extentd at boot), putting the dead writer's creates between the new recovery and its fence; without it wal_after_fence's first seed was 848 (70 s), 750 to 2653 under other weights.
Blind spot closed: a commit applied twice (a segment restamped though it landed) left the branch's contents, root and spent as they were, and no check saw it. New oracle in World::check, the spec's HistoriesOwn: the branch's applied segments since its newest checkpoint, walked back from state.last_segment by footers' after, apply each commit after its truth parent (the first after the base when there is no checkpoint). It reads each footer once ((branch, seq, epoch) -> (after, commits), one writer stamping one footer at a place): uncached it tripled every seed's cost (recoveries_need_newest_checkpoints 109 s in release vs 39 s on 3309abe89), cached 37 s; every rule test finds the same seed as on 3309abe89 (2889, 11464, 342, 2928 for the slow four).
Rule tests: logs_need_to_follow (wals, seed 2: an acknowledged commit lost, or a fork made twice), logs_need_the_end_after_the_fence (wals, seed 182: 'applies commit 17 after Some(32), not its parent Some(16)', the double application the spec's NoWalAfterFence predicts). First try found wal_after_fence only through a sim artifact (rebase set the base to the pre-fence head with the rule off, so the guest started behind the branch); fixed in Wal::rebase: base is the branch's after the fence.
Structure: the sim's old re-batching (World::opened) was a second, simulation-only implementation of what Wal::rebase now does once, for production and the sim alike.
Spec: Bucket.tla's header comment only (the two WAL rules are now Rules fields). doc-14 step 4 says how it is implemented; OPEN_QUESTIONS.md # Decisions has the fork-alone placement and the sim's choices. AC #1 reworded to by commits, as doc-14 has it since TASK-165.2.
Overlap with collector-fixes (TASK-173/174/183): no collector or deletion code touched; both branches edit lib.rs's Rules (a field each, if theirs adds deletions_fence), sim/mod.rs (one()'s rule names, profiles, test lists) and possibly sim/world.rs: textual conflicts only.

Verified 2026-11-05 (branch task-165-5, on 75bd872ba).
- extent_remote, macOS debug: 209/209 (396 s, after the footer cache; 747 s before it, 209/209 too).
- Rule tests, release: all 23 test_needs pass (logs_need_to_follow seed 2, logs_need_the_end_after_the_fence seed 182); the slow four find the same seeds as 3309abe89.
- Soak, release, SIM_SOAK=100 of the 16 holds tests (0..20000 each, everything 0..30000): 16/16, before the footer cache (1249 s) and after it (799 s).
- scripts/check-tla.sh, the bucket spec (its header comment changed): all 11 models hold, all 30 mutations give their counterexamples (NoWalFollows: NoAckedLost; NoWalAfterFence: HistoriesOwn). Run in two parts: the whole script was stopped at the 30-minute background limit after 8 models had passed, the machine loaded; the rest run by name. follower and dax-cow not rerun (unchanged).
- macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: exit 0 (workspace 2134/2134; shuttle parts all green). Before the footer cache, which touches only the simulation.
- Linux, Docker (OrbStack, rust:1 arm64), cargo nextest run -p extent_remote -p extent_store -p fxdax: 893/901, before the footer cache. The 8 failures: eu-stack (stacks_name_where_threads_wait, fxdaxctl_ and the_daemon_writes_its_stacks_on_sigquit), grants ENOENT (3), the build stamp, and seccomp's copy_on_write_windows_run_under_firecrackers_seccomp_filters, because this worktree had vendor/firecracker checked out at a2db2b3b0, not main's d53645971; after git submodule update it passes.
- scripts/upstream-changes.py --check: exit 0.
Not run: GCE (scripts/test-gce.sh), DoD #1's Linux half.
<!-- SECTION:NOTES:END -->
