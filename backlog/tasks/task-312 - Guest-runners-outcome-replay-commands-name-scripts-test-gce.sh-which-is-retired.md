---
id: TASK-312
title: >-
  Guest runner's outcome replay commands name scripts/test-gce.sh, which is
  retired
status: To Do
assignee: []
created_date: '2026-11-10 00:23'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 720000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/tests/runner/src/main.rs's replay() writes each round's replay command as 'FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=<case> ... scripts/test-gce.sh all' into round-K.json (outcome_json's "replay"), and the tests an_outcome_says_how_to_replay_it and an_outcome_of_no_run pin it. GCE test hosts are retired (Linux testing runs on ubu1 with scripts/test-e2e.sh), so the command replays nothing. Found while working TASK-311.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 replay names a command that runs on the current Linux test host (scripts/test-e2e.sh <case> with the knobs as env)
- [ ] #2 The runner's tests pin the new command
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
