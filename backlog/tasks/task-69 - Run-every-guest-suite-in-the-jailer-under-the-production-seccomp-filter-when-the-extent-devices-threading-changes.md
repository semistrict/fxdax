---
id: TASK-69
title: >-
  Run every guest suite in the jailer under the production seccomp filter when
  the extent device's threading changes
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: medium
ordinal: 178000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
0a6f1e33 moved request completion onto the reply thread; only fxfs-guest-dax ran under the production filter. Rerun all 26 suites in the jailer whenever the extent device's threading changes (and now).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
