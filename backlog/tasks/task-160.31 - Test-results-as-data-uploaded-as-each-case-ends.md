---
id: TASK-160.31
title: 'Test results as data, uploaded as each case ends'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 10:24'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 602000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A mutant's fate is a grep for ': failed in round ' (any failure kills); replay knobs are scattered; results are fetched once at the end, lost if the host goes. Explains b0ccb059c, 19031ef69, fd293cec3, ad8aeef22, 126298d12. Proposal: outcome.json per case and round (outcome, failed checks, every knob, commit, replay command), summary.json read by fate(), the gate and soak; each case uploaded to the run's bucket prefix as it finishes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 outcome.json per case; fate() reads it
- [ ] #2 A host killed mid-run keeps its finished cases
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Runner: test --outcome PATH --run RUN.json --round K writes the round's outcome as JSON (outcome, reason, failed checks from the guest's FAILED: line, took/limit, commit, knobs of the run plus FXDAX_SEED from the case's seed file, replay command).
2. scripts/guest_results.py: start (run.json: commit, knobs, rounds, cases), ended (summary.json, upload), finish (status, logs), report, fetch; FXDAX_RESULTS_BUCKET uploads run.json, summary.json, cases/CASE.tar.gz, logs.tar.gz.
3. test-e2e.sh: one loop for one round or several, each writing CASE/round-K.json, ended after each round, finish at the end. test-linux-host.sh passes FXDAX_RESULTS_BUCKET/e2e; test-gce.sh gives the run's prefix and fetches from it when the host gave no summary; gce.sh grants storage scope.
4. Readers: guest-mutants-gce.py fate(status, summary), dax-gate.sh and soak-workloads-gce.sh print report.
5. Tests: runner unit tests (failed_checks, shell_word, outcome/replay); test_guest_results.py (summary through rounds, report, a lost host's finished cases fetched back, a failing upload); fate tests; fake gcloud storage cp.
6. Verify: Docker test.sh parts; GCE r605 (the given run), r606 with a bucket and the host deleted mid-run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: one writer per file. The runner owns a round's outcome (`outcome_json`), scripts/guest_results.py the run's (run.json, summary.json) and the bucket's layout both ways; readers go through guest_results.load/report. Round outcomes are CASE/round-K.json, not outcome.json: a soak's passing rounds keep theirs beside round-K.log after their directories go. test-e2e.sh's two loops (one round, several) became one. Knobs: every FXDAX_*/PJDFSTEST_* at test-e2e.sh's start but where it writes (WHERE in guest_results.py) and a host-built kernel's path; FXDAX_SEED from the seed file the checks and script cases leave. Replay: FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=CASE <knobs> scripts/test-gce.sh all. Failed checks come from the checks guest's `FAILED: a, b` line; TASK-160.30 must keep it or hand the runner names another way (OPEN_QUESTIONS.md).

Blind spot: the drivers grepped e2e.log for ': failed in round ', which each change of wording or of how rounds ran broke (b0ccb059c, 19031ef69); results came back only at the end, by one tar over SSH, gone with the host. fate() now reads summary.json: killed iff a case failed or timed out; status 0 MISSED; else BROKEN (DAX refusals alone stay BROKEN, as before).

Structure: fine; the seam between runner and scripts is now the outcome file, not log text.

Verified: Docker test.sh parts (clippy, scripts 159, test_cases 63+60, runner 4). GCE r605: three cases, each CASE/round-1.json (outcome, took/limit, commit, knobs with FXDAX_SEED for the checks cases, replay), summary.json status 0; guest tests exit 0. r606 (FXDAX_RESULTS_BUCKET, us-east1-b): host deleted after its first case landed; the last two finished within seconds first, so the driver found the host gone before fetching and got all three cases, logs and summary back from the bucket (status 0). r607: my wait timed out and deleted the host before any case ended; the bucket's run.json and summary (both cases unrun) came back. r608: FXDAX_E2E_ROUNDS=2, host deleted once checks-dax's round 1 landed: fetched checks-dax/round-1.json and round-1.log, summary 'stopped after round 1 of 2, which passed', fxfs-guest-dax not run, 'the run did not finish'; test-gce.sh exit 1 (AC #2). Outcomes say commit fc58056f1-dirty: the backlog notes I edited while the source was pushed. The bucket prefix used (gs://echophase-fxdax-bench/results-t1629) is deleted. Unrun: a failing case on GCE (failed_checks and a fate of killed are covered by unit tests only), the dax gate, the soak and the mutants script against real hosts.
<!-- SECTION:NOTES:END -->
