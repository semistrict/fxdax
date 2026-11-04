---
id: TASK-150
title: >-
  Bugs found on 2026-11-01: confirm each with a test, fix it, close its blind
  spot
status: Done
assignee: []
created_date: '2026-11-01 16:49'
updated_date: '2026-11-04 07:07'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 515000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Two static audits (extentd, stores and VMM; guest kernel Fxfs and DAX) each reported seven traced bugs, none yet run, and GCE runs r394 and r395 failed in ways not yet diagnosed. A finding is real only once a test goes red: one that can't be made to fail is closed as not a bug, saying why. Each that holds goes through AGENTS.md's blind spot rule.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every subtask is fixed, or closed as not a bug with the test that shows it
- [ ] #2 Their blind spots are ranked into TASK-149
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
All 23 findings of 2026-11-01 are fixed, explained or made moot, each with its test, and each blind spot ranked into TASK-149.
<!-- SECTION:FINAL_SUMMARY:END -->
