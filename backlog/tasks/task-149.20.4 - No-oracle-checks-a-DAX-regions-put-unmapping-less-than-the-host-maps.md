---
id: TASK-149.20.4
title: No oracle checks a DAX region's put unmapping less than the host maps
status: To Do
assignee: []
created_date: '2026-11-07 13:37'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: medium
type: task
ordinal: 669000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20 reading guest/mutants/task-44.19-map-to-unlocked.patch (Killer: none): copying on write, each range MAPs every page it uses, and what is left of the mutant is a region's put UNMAPping less than the host maps, which no oracle in guest/sim or the guest suites checks. A leak of host mappings (vm.max_map_count, the daemon's window state) would pass every test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An oracle (guest/sim's, or the daemon's window state at a check's end) compares what the guest unmapped with what the host maps
- [ ] #2 task-44.19-map-to-unlocked gets a killer, or its header says why none can
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
