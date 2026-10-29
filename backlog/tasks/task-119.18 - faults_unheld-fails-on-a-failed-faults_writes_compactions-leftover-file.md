---
id: TASK-119.18
title: faults_unheld fails on a failed faults_writes_compaction's leftover file
status: Done
assignee: []
created_date: '2026-10-28 22:59'
labels: []
dependencies: []
parent_task_id: TASK-119
ordinal: 458000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
faults_unheld reruns faults_writes_compaction with its file name. A failed run leaves the file, which the rerun reopened without truncating; its model starts empty, so leftover bytes read as lost stores. A failed check also left fxfs parameters (dax_fail_held, dax_evacuates, dax_fault_seals) set for the checks after it.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Checks truncate model files at open; a Param guard puts fxfs module parameters back on drop, panics included (776cfbe3f).
<!-- SECTION:FINAL_SUMMARY:END -->
