---
id: TASK-119.8
title: >-
  DAX gate: script plus pre-push hook; DAX changes pass the checks, delay
  rounds, torture, crash and every hand mutant
status: In Progress
assignee: []
created_date: '2026-10-28 19:27'
updated_date: '2026-10-29 07:01'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29: AC3's two survivors get deterministic checks from pause points (TASK-119.9): evacuation_paused for evacuation_unlocked, prune_while_paused for prune_in_use (branch t3/task-112-6; GCE r214, r215 pending). The gate's suites take checks-dax-small and the crash-at-step variants. TASK-130 found fresh hosts skipped every guest case and passed: the gate's runs set FXDAX_GUEST_KERNEL_BUILD=release, which main now defaults to.
<!-- SECTION:NOTES:END -->
