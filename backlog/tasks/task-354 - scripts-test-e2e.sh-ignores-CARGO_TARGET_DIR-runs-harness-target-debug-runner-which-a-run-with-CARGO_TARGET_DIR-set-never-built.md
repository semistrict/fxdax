---
id: TASK-354
title: >-
  scripts/test-e2e.sh ignores CARGO_TARGET_DIR: runs
  harness/target/debug/runner, which a run with CARGO_TARGET_DIR set never built
status: To Do
assignee: []
created_date: '2026-11-11 12:36'
labels: []
dependencies: []
ordinal: 713000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-11), CARGO_TARGET_DIR=~/src/fxdax-t350-target scripts/test-e2e.sh built the runner into that directory and then failed: 'guest/tests/target/debug/runner: No such file or directory' (line 51; FXDAX_GUEST_AGENT at line 177 has the same path). Memory said TASK-305 made test-e2e.sh honour CARGO_TARGET_DIR.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 test-e2e.sh runs with CARGO_TARGET_DIR set, finding the runner and the guest agent there
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
