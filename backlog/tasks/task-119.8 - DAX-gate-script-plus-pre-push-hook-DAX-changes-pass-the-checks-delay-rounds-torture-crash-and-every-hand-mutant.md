---
id: TASK-119.8
title: >-
  DAX gate: script plus pre-push hook; DAX changes pass the checks, delay
  rounds, torture, crash and every hand mutant
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 444000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
There is no CI and nothing gates a push. Mutants prune_in_use (never killed) and evacuation_unlocked (killed ~1 in 70 rounds) show the suites miss rules.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/dax-gate.sh runs on GCE and records a pass for a commit
- [ ] #2 A pre-push hook refuses changes under guest/linux/fs/fxfs/ without a pass
- [ ] #3 Every hand mutant is killed within 3 rounds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
