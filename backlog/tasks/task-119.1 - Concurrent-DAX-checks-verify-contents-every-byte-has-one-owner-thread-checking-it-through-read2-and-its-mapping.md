---
id: TASK-119.1
title: >-
  Concurrent DAX checks verify contents: every byte has one owner thread
  checking it through read(2) and its mapping
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 437000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
seal_race, evacuee_waited, log_writes, faults_writes_compaction, faults_unheld, faults_seal_holding_the_semaphore and append_while_mapped check no contents, or only after a quiet final rewrite (test_fxfs_guest_checks.rs:775), so a write lost mid-race goes unseen. append_writeback checks only the length after eviction.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each listed check asserts exact contents during the race, not only at the end
- [ ] #2 append_writeback checks contents after eviction
- [ ] #3 A hand mutant of dax.c that loses a write fails each check it should (named per check)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
