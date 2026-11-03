---
id: TASK-160.28
title: Scheduled hunts with rotating seeds
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
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
