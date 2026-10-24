---
id: TASK-100
title: >-
  fxdax::memory tenants_past_their_limits_evict_then_refuse aborts (SIGABRT)
  intermittently on Linux
status: To Do
assignee: []
created_date: '2026-10-24 20:28'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 386000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r70 (2026-10-24, us-central1-a, main at 42cd8328c plus a test): SIGABRT after 1.5 s, no panic message, stderr only memory-pressure's startup lines. Passed in r65 the same day (2.1 s). An abort without a panic: a double panic, an allocation failure, or a Rust abort on OOM; the test drives tenants past their memory limits.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
