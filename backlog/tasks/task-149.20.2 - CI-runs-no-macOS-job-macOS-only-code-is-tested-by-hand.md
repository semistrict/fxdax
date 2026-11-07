---
id: TASK-149.20.2
title: 'CI runs no macOS job: macOS-only code is tested by hand'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 13:36'
updated_date: '2026-11-07 14:51'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A CI job, macos, on GitHub's macos-15 runners (arm64): scripts/test.sh's workspace and shuttle parts, a matrix of the two, as the DoD's macOS runs are a laptop's; with the submodule, nextest and a cache of its own.
2. Within macOS's default open files (256): extent_store, extent_remote, fxdax and physmem pass so locally, so no raise.
3. test_ci.py checks the job runs those parts on macOS.
4. Push land-h6, CI's macos job the proof; what fails there and not on a laptop is fixed or filed.
<!-- SECTION:PLAN:END -->
