---
id: TASK-339
title: >-
  Flaky on macOS CI:
  remote::tests::forks_read_what_they_inherited_compacted_twice, a
  once-compacted moves key still listed after the collector's fourth pass
status: To Do
assignee: []
created_date: '2026-11-11 00:57'
labels:
  - testing
  - extent_remote
dependencies: []
priority: medium
type: bug
ordinal: 739000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42932581818 (branch t330-land at cd307c317, macos (workspace) job 130234811891, 2026-11-11): 2227 of 2228 passed; this one failed at crates/fxdax/src/remote.rs:559, assert!(!keys.iter().any(|key| key.starts_with(&once))): the listing held 'moves/moves/segments/3cc4a6f15455bb07/0000000000000001' and still 'moves/segments/3cc4a6f15455bb07/0000000000000001'. The same commit's code (no fxdax crate change from main) passed it in CI runs 42915833387 and 42924964007 and in two local macOS suites. So the moves compacted once sometimes outlive collect(4): the test's collections race something (the writer's checkpoint upload, the uploader's interval) or the collector's keep rule (TASK-195's) differs by timing. Not reproduced yet.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
