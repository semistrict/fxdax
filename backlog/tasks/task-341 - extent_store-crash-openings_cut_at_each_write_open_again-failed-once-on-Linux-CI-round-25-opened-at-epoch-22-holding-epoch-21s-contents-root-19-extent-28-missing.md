---
id: TASK-341
title: >-
  extent_store::crash openings_cut_at_each_write_open_again failed once on Linux
  CI: round 25 opened at epoch 22 holding epoch 21's contents (root 19, extent
  28 missing)
status: To Do
assignee: []
created_date: '2026-11-11 01:22'
updated_date: '2026-11-11 10:07'
labels:
  - extent_store
  - durability
dependencies: []
priority: high
type: bug
ordinal: 740000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42935580033, test (workspace) job 130244855227 (branch t330-land at d0beb3c9d, whose extent_store is main's 7e1740cb7, 2026-11-11): 2483 of 2484 passed; this one failed at crates/extent_store/tests/crash/kept.rs:318, 'round 25: at epoch 22': the store, cut at a write and opened again, said epoch 22 but held [... (27, 7, 49673)] root 'root 19', where epoch 22's contents add (28, 5, 14245) and root 'root 21'. So an opening after a cut claimed an epoch whose commit's data it lacks: either a commit acknowledged durable that recovery lost, or the epoch counted past the contents. The same code passed in CI runs 42915833387, 42924964007 and 42932581818 and on ubu1; the test's cuts or timings vary by run (it printed no seed). Recent changes in the area: TASK-92.41 (commits spanning segments, a5dd824c5, ca1c208b7), TASK-318. Not reproduced yet; first step: find what the test's randomness is and make a failing round replayable.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Again, CI run 42979578798 (t330-land rebased on main dfd9eb484), test (workspace) job 130386571778: extent_store::crash recovers_from_its_bucket_with_and_without_its_disk failed at the same kept.rs:318, 'round 25: at epoch 23', the same contents, (28, 5, 14245) missing, root 'root 23'. The rounds' contents are the same each run (the same extents, round 25 both times), so the cut or the timing varies. Two failures in two consecutive Linux CI runs of the crash tests.

Reproduced on ubu1 with main 7e1740cb7's extent_store (no TASK-330.1 change in it): cargo nextest run -p extent_store --test crash, 3 runs, one failed: recovers_from_its_bucket_with_and_without_its_disk, 'round 81: at epoch 10' (kept.rs:318). About 1 in 3 runs of the crash tests here, 3 of 4 Linux CI runs of t330-land. Main's own CI passed it in its last runs by luck.

Third and fourth Linux CI hits on t330-land (run 42979578798, its reruns): openings_cut_at_each_write_open_again, 'round 25: at epoch 23'. It now fails most Linux CI runs: every push's CI is a coin toss until it's fixed.
<!-- SECTION:NOTES:END -->
