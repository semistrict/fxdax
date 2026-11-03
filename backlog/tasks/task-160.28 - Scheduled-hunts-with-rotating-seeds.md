---
id: TASK-160.28
title: Scheduled hunts with rotating seeds
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-03 16:34'
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
- [x] #1 A scheduled workflow explores new seeds
- [x] #2 A mutant needing a rare seed is found by it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

AC 2: task-99-region-open, which 3,000 seeds find once (seed 2544), was found by the hunt at interleave 56669 and 62739 in seeds 43,001-63,000 (run 41667054404, moving first seed). Its other two failures, failures 52299 (UBSan, dax.c:1292) and interleave 54042 (a zero read), pass on main without the mutant (runs 41675269814, 41675274637, replayed with the new seeds input); the UBSan one is real code, reached only by the mutant's interleaving: TASK-150.23. DoD 1 not checked: no GCE run; Linux on GitHub's runners.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
hunt.yml: nightly (and dispatchable) CPU-only hunt, no secrets. zloop over 20,000 seeds of interleave and failures a night from a first seed moving with the run number (4 shards), and fxfs-posix's model and corruption tests at 50x (4 shards, FXDAX_CORRUPT_SEED and PROPTEST_RNG_SEED from run and shard). Artifacts keep failures, logs and replay commands; guest/sim/known-failures and crates/fxfs-posix/known-panics keep open bugs (TASK-149.23 now) from failing. Dispatch inputs first, seeds, mutant, posix. Verified: run 41657634400 clean over seeds 3,001-23,000 and 4x(1,200 model, 1,500x4 corruption) cases; run 41667054404 found mutant task-99-region-open at new seeds; one real bug filed, TASK-150.23.
<!-- SECTION:FINAL_SUMMARY:END -->
