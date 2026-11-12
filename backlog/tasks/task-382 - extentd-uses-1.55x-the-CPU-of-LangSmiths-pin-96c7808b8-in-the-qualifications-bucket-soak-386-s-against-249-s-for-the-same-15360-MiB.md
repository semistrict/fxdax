---
id: TASK-382
title: >-
  extentd uses 1.55x the CPU of LangSmith's pin (96c7808b8) in the
  qualification's bucket soak: 386 s against 249 s for the same 15,360 MiB
status: To Do
assignee: []
created_date: '2026-11-12 11:36'
labels:
  - performance
  - bug
dependencies: []
priority: medium
ordinal: 761000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-330.4's A/B on ubu1 (docs/measurements/ubu1-qualify-ab-2026-11-12, clean rounds only, two per side, one in each order): in the bucket soak, main's daemon used 386 s of CPU, the pin's 249 s, while main wrote half the bytes (15.20 against 30.25 GiB) and used 0.38x the memory. Nothing checks a daemon's CPU, so the qualification passed it silently. On a sandboxes host the daemon's CPU competes with the guests' vCPUs (TASK-336: framings' write phase slowed on an overcommitted host). Find where main spends the extra CPU (perf record of the daemon in the soak, main against the pin: compression of every segment now framed in place or in a copy since TASK-92.41/310, bucket-fault retries, the make-durable thread), then fix it or justify it, and add a daemon CPU budget to scripts/qualify-budgets.py so the qualification fails a regression.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where main's extra soak CPU goes is measured (perf profile against the pin, the top differences named)
- [ ] #2 The extra is removed or justified in the task's notes with the profile
- [ ] #3 The qualification checks the daemon's CPU per guest MiB against a budget and fails past it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
