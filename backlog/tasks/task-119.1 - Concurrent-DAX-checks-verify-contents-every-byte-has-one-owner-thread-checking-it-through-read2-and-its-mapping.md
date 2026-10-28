---
id: TASK-119.1
title: >-
  Concurrent DAX checks verify contents: every byte has one owner thread
  checking it through read(2) and its mapping
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Byte-owner checks (b9af10794, branch test/119): seal_race (each round, own byte via map and read(2), other's byte monotonic), log_writes (each write read back; whole file at end), faults_writes_compaction/faults_unheld (write(2)s own pages' first halves, stores second halves; periodic read-back; full model at end), faults_seal_holding (stored bytes modelled, read back periodically and at end), append_while_mapped (mapped pages hold what was appended; whole file at end), append_writeback (contents after evict). Verify on GCE r206 (checks-dax x3, checks-dax-delay x3); then hand mutants.
<!-- SECTION:PLAN:END -->
