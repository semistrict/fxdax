---
id: TASK-172
title: >-
  A jailed VMM sizes its DAX run budget from vm.max_map_count's default, not the
  host's: it can't read /proc
status: To Do
assignee: []
created_date: '2026-11-05 07:06'
labels:
  - dax
  - jail
  - perf
dependencies: []
priority: medium
ordinal: 635000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
uffd.rs's default_budget reads /proc/sys/vm/max_map_count, which the jailer's chroot lacks, and falls back to 65,530: a budget of 30,717 runs where the GCE hosts' 1,048,576 gives 522,240. The jailed VMM trims runs about seventeen times as often as an unjailed one, which made TASK-170's leak common (21,396 pages in one pgbench run) and costs faults. Second jail-only /proc dependency after TASK-167 (thread names): the VMM's view of the host shouldn't depend on /proc it may not have.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A jailed VMM's budget is the host's, as an unjailed one's is, or a configured one (e.g. given in the extent device's configuration or by the jailer's caller)
- [ ] #2 A test shows the budget without /proc is what was given, not the default
- [ ] #3 No other read of /proc in the VMM's DAX code silently falls back in the jail (audit uffd.rs and the extent device)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
