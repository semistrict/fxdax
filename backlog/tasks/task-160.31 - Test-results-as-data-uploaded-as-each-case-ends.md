---
id: TASK-160.31
title: 'Test results as data, uploaded as each case ends'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
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
