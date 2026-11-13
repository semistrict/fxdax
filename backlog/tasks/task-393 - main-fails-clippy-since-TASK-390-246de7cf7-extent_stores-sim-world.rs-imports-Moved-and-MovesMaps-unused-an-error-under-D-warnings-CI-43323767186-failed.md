---
id: TASK-393
title: >-
  main fails clippy since TASK-390 (246de7cf7): extent_store's sim/world.rs
  imports Moved and MovesMaps, unused, an error under -D warnings (CI
  43323767186 failed)
status: Done
assignee: []
created_date: '2026-11-13 00:04'
updated_date: '2026-11-13 01:40'
labels:
  - testing
  - bug
dependencies: []
priority: high
ordinal: 770000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-390 moved what used them into extent_remote::testing; scripts/test.sh's clippy part, and CI's, fail on the unused imports. Found running TASK-369's suite.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
main's clippy failed on 246de7cf7 (TASK-390), CI 43323767186. Found running TASK-369's suite; f926e1488 on main fixed it (extent_store's sim/world.rs imports nothing unused) before TASK-369's branch landed, which carried the same fix until its rebase.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed on main by f926e1488: the unused imports went; main's clippy passes again.
<!-- SECTION:FINAL_SUMMARY:END -->
