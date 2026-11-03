---
id: TASK-160.28
title: Scheduled hunts with rotating seeds
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-03 15:45'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 599000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every push replays the same 3,000 zloop seeds, and the model and corruption tests run at 96 cases; hunts are by hand (TASK-149.23 surfaced only in a 4,000-case run). Proposal: a scheduled CPU-only workflow: zloop from a moving first seed over interleave and failures, the model and corruption tests at 50x, failing seeds kept with their replay command, a list of known signatures so open bugs don't fail every run.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A scheduled workflow explores new seeds
- [ ] #2 A mutant needing a rare seed is found by it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. hunt.yml: nightly + workflow_dispatch, CPU only, no secrets.
2. zloop: 4 shards x 5,000 seeds of interleave and failures from 3001 + 20,000 per run number; -p puts a mutant in (dispatch input) to prove a hunt finds one CI's 3,000 don't.
3. fxfs-posix model/corrupt tests at 50x (4 shards: FXDAX_MODEL_CASES 1200, FXDAX_CORRUPT_CASES 1500), FXDAX_CORRUPT_SEED and PROPTEST_RNG_SEED from run number and shard.
4. Artifacts: logs, failures, replay commands; known signatures (guest/sim/known-failures, crates/fxfs-posix/known-panics) keep open bugs from failing.
5. Dispatch once; file new bugs with seeds.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed on main 259476d8b: .github/workflows/hunt.yml, nightly 03:00 UTC + workflow_dispatch (first, mutant, posix). zloop: 4 shards x 5,000 seeds of interleave and failures from 3001 + 20,000 per run number before; fxfs-posix: 4 shards, FXDAX_MODEL_CASES 1200 and FXDAX_CORRUPT_CASES 1500 each (50x 96 and 120), FXDAX_CORRUPT_SEED 120 + (run-1)*4+shard)*1500, PROPTEST_RNG_SEED (run-1)*4+shard+1. Artifacts zloop-N (failures, replay, logs, README with the commit) and posix-N (nextest.log, replay). Known: guest/sim/known-failures (regex of zloop's failure line; TASK-150.21's entry went with its fix on main), crates/fxfs-posix/known-panics (TASK-149.23's reader.rs:159, counted apart by corrupt_tests; seeds 700-759 pass with it, seed 741 hits it). Run 41657634400 (dispatch, as the schedule runs): seeds 3,001-23,000 of both clean, ~45 min a shard; fxfs-posix 4,800 model sequences and 6,000 corruption cases each image, all pass, ~50 min a shard, no panics, TASK-149.23's not reached in 120-6,119. Run 41657639663, mutant task-153.3-refused-slice-stale, seeds 1-20,000 of both: not found (README said none in 3,000 either): out of seeds' reach now.

Correction: run 41657634400's passing tests' output wasn't kept, so whether a known panic showed there is unknown; the hunt now runs nextest with --success-output final, which shows it (checked locally: seeds 700-759 print 'known: TASK-149.23').
<!-- SECTION:NOTES:END -->
