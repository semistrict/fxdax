---
id: TASK-320
title: >-
  fxdax examples are measured with fxdax's failpoints and testing features
  compiled in
status: To Do
assignee: []
created_date: '2026-11-10 09:49'
labels:
  - bug
  - perf
dependencies: []
priority: medium
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/Cargo.toml has a dev-dependency on itself with features ["failpoints", "testing"], and on extent_store with "testing". Examples are built with their package's dev-dependencies, so every fxdax example (extentd-mapbench, import-bench, store-read, store-bench, filesystem-io-bench, fxfs-metabench, pagein-bench) runs with fail/failpoints on, so each fail_point! in the daemon, extent_store and extent_remote looks up the fail crate's global registry under its RwLock, and with extent_store's `testing` lock instrumentation. Same class of leak as TASK-303 (the audit), which a release build now refuses; this one is not guarded. Found while fixing TASK-303.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A release build of a measured example has neither fail/failpoints nor extent_store/testing, shown by a build check or a guard like TASK-303's
- [ ] #2 The cost of the failpoints in a measured example is measured once (e.g. extentd-mapbench with and without), and the result recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
