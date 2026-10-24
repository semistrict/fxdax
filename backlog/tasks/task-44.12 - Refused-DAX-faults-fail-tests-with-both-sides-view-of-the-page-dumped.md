---
id: TASK-44.12
title: 'Refused DAX faults fail tests, with both sides'' view of the page dumped'
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 391000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A store the host refuses, the guest believing the page writable, stops the VM a second later with only KVM_RUN's EFAULT (TASK-99); the sandbox team had no logs to give. The host now logs refusals (extentd's describe_window_offset, the VMM's refused()). A refusal that repeats is an invariant broken: guest belief and host truth differ. Tests should fail on it at once, with what the guest thinks of the page (its region, range bits, PTE) beside what extentd maps.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest debug interface reports its view of a window offset: the region or range, its writable and mapped state
- [ ] #2 In tests, a refusal repeated for one page fails the run with both views logged
- [ ] #3 Test runs assert no refusals past those a seal race explains, from extentd's and the VMM's counters
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
