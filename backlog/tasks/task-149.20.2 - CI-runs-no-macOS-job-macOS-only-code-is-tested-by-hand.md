---
id: TASK-149.20.2
title: 'CI runs no macOS job: macOS-only code is tested by hand'
status: To Do
assignee: []
created_date: '2026-11-07 13:36'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: medium
type: task
ordinal: 667000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. CI (.github/workflows/ci.yml) runs on ubuntu alone; the DoD's macOS runs are a laptop's. Code compiled for macOS alone, and its tests, run nowhere else: physmem's macOS physical map (crates/physmem/src/physmap.rs, tests/config.rs), extent_store's macOS syncs (crates/extent_store/src/dir.rs, F_FULLFSYNC, TASK-92.48), extent_remote's cache (crates/extent_remote/src/cache.rs), fxdax's storage I/O (crates/fxdax/src/storage/io.rs, not(linux)). A regression there lands green.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A CI job runs the workspace's tests on macOS (GitHub's macos runners), or the user decides against it, recorded in OPEN_QUESTIONS.md
- [ ] #2 It passes on main, or what fails is filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
