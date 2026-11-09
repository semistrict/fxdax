---
id: TASK-305
title: >-
  scripts/test-e2e.sh ignores CARGO_TARGET_DIR: it runs binaries from target/
  paths the build didn't write
status: To Do
assignee: []
created_date: '2026-11-09 13:01'
labels:
  - bug
  - scripts
dependencies: []
priority: medium
ordinal: 715000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-09, main 027a3926), CARGO_TARGET_DIR=$HOME/src/fxdax-t292-target scripts/test-e2e.sh fxfs-guest-checks-dax-delay failed at once: 'scripts/test-e2e.sh: line 80: .../guest/tests/target/debug/runner: No such file or directory' (exit 127). The script hardcodes $harness/target/debug/runner, $root/target/release/fxdax-extentd, fxdaxctl and fxdax-gc, and $harness/target/$guest_target/debug/guest-agent, while cargo builds into CARGO_TARGET_DIR when it is set; ubu1's rule is a target dir per worktree. Resolve the paths from cargo (cargo metadata's target_directory, or build with --message-format and take the artifacts), for the root workspace and guest/tests alike.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/test-e2e.sh runs a case with CARGO_TARGET_DIR set, the binaries taken from it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
