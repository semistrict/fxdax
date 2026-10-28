---
id: TASK-119.3
title: >-
  Delay injection in the default suites: fxfs-guest-checks-dax-delay with a seed
  per round; DAX diagnostics fail tests
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 19:27'
updated_date: '2026-10-28 19:36'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 439000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs.dax_delay_us found TASK-101, 112.6 and 113, but no test or script sets it; runs were by hand via FXDAX_GUEST_CMDLINE. dax_evacuee_verify warns with pr_warn, which the runner does not count as a failure.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A case runs checks-dax with fxfs.dax_delay_us=200, fxfs.dax_evacuee_verify=1 and a seed per round, in the default e2e and GCE sets
- [ ] #2 Delay points at seal begin and end and the evacuation steps
- [ ] #3 A DAX diagnostic warning fails the test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
New case fxfs-guest-checks-dax-delay (fxfs.dax_delay_us=200, dax_delay_seed = the run's seed) in test-e2e.sh defaults, test-guest-kernel.sh and the mutant suites; Vm::module_param. New delay points: fxfs_dax_seal between regions and files, fxfs_dax_sealed, evacuation between MAP and place. AC3 (diagnostic warnings fail tests): dax_evacuee_verify lives on wip/task-112-5 only; it becomes a WARN there.
<!-- SECTION:PLAN:END -->
