---
id: TASK-274
title: >-
  guest/tests clippy fails on newer Rust: four lints in script.rs,
  test_fs_bench.rs and vm.rs
status: To Do
assignee: []
created_date: '2026-11-09 03:26'
labels: []
dependencies: []
ordinal: 701000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-273's agent: cargo clippy on the guest test crate (guest/tests) in a rust:1 container with a newer toolchain than CI's reports four lint errors in code TASK-273 did not change: guest/tests/test_cases/src/script.rs, test_fs_bench.rs, and an existing layout in vm.rs. CI will hit them when its toolchain moves.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo clippy on guest/tests is clean with the current stable Rust
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
