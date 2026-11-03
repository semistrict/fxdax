---
id: TASK-160.28
title: Scheduled hunts with rotating seeds
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-03 13:29'
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
