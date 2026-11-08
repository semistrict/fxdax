---
id: TASK-265
title: >-
  rustfmt drift on main: extent_store and fuchsia-async unformatted, and CI does
  not check formatting
status: To Do
assignee: []
created_date: '2026-11-08 20:32'
labels: []
dependencies: []
ordinal: 695000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
cargo fmt --all --check on main at 4f336f788 reports diffs in crates/extent_store/src/seals.rs, src/store.rs, tests/store.rs and crates/fuchsia-async/src/runtime/scope.rs (plus vendored guest/rust/std, which should be excluded rather than reformatted). No CI job runs rustfmt, so drift lands unnoticed. Found by TASK-264's agent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo fmt --all --check is clean for fxdax's own crates
- [ ] #2 CI fails on unformatted code, vendored trees excluded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
