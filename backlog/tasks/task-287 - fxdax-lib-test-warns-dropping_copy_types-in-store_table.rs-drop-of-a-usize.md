---
id: TASK-287
title: fxdax lib test warns dropping_copy_types in store_table.rs (drop of a usize)
status: To Do
assignee: []
created_date: '2026-11-09 08:01'
labels:
  - bug
dependencies: []
priority: low
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/test.sh on macOS (TASK-257's run, main 142195ae2): warning: calls to std::mem::drop with a value that implements Copy does nothing, crates/fxdax/src/store_table.rs: hold.hold().retire(|| drop(store.cuts.fetch_add(1, Ordering::SeqCst))). Use let _ = or a block.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No dropping_copy_types warning in the workspace's test build
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
