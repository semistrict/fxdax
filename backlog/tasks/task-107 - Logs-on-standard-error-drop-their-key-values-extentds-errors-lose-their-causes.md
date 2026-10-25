---
id: TASK-107
title: >-
  Logs on standard error drop their key-values: extentd's errors lose their
  causes
status: Done
assignee:
  - '@claude'
created_date: '2026-10-25 18:56'
updated_date: '2026-10-25 19:25'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 406000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Logs written with key-values (`fxfs::log::error!(range:? = unit, error:?; "Failed to load an extent's range")`, some 25 call sites) reach standard error as the message alone: fxdax-extentd's logger, the two test harnesses' (copies of it) and diagnostics-log's print `record.args()` only, though the workspace builds `log` with `kv`. TASK-106's abort logged that a page-in failed but not why, so the flake cannot be diagnosed from its log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A record's key-values follow its message on standard error, in fxdax-extentd, in fxdax's tests and through diagnostics-log
- [x] #2 fxdax has one standard-error logger, which the daemon and its tests share
- [x] #3 A test asserts the line a record with key-values is written as
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One standard-error logger in fxdax (extentd::logging::to_stderr), its line the record's level, target, message, then key=value each; the daemon binary and both test harnesses use it instead of three copies.
2. diagnostics-log's StderrLogger writes key-values likewise.
3. Unit tests of each line format; Linux workspace on GCE (r100).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r100: Linux workspace 2634/2634 (extentd::logging::tests both green); macOS fxdax + diagnostics-log 229/229; upstream-changes --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Records' key-values now follow their message on standard error: fxdax has one logger, extentd::logging::to_stderr, which fxdax-extentd and both test harnesses use in place of three copies that printed the message alone; diagnostics-log writes key-values too. Unit tests of each line (Linux r100, macOS); full Linux workspace green. TASK-106's next abort will name its cause.
<!-- SECTION:FINAL_SUMMARY:END -->
