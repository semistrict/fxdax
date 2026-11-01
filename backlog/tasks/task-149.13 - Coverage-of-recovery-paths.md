---
id: TASK-149.13
title: Coverage of recovery paths
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 514000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Tests can't find bugs in code they don't reach. Replay, mount and error paths are where the bugs of 2026-10-31 were. Verify cargo-llvm-cov's license before adding it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A report shows line coverage of journal, allocator and replay under the model test
- [ ] #2 Paths no test reaches are filed or covered
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
