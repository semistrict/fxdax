---
id: TASK-305
title: >-
  scripts/test-e2e.sh ignores CARGO_TARGET_DIR: it runs binaries from target/
  paths the build didn't write
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 13:01'
updated_date: '2026-11-10 09:30'
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
- [x] #1 scripts/test-e2e.sh runs a case with CARGO_TARGET_DIR set, the binaries taken from it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. test-e2e.sh: target_dir DIR [CARGO], cargo metadata's target_directory, run where each build runs (its workspace's directory, so its .cargo/config.toml applies).
2. Runner, guest agent (guest/tests), extentd/fxdaxctl/fxdax-gc (root workspace; built from $root now, not --manifest-path from the caller's directory), Firecracker and jailer (through firecracker-cargo.sh, whose tree's config sets build/cargo_target unless CARGO_TARGET_DIR does) all from it.
3. Verify on ubu1 as ramon: CARGO_TARGET_DIR=<own dir> scripts/test-e2e.sh <case>, the binaries from that dir, nothing under the worktree's target/ but target/firecracker's source copy.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done as planned. Verified on ubu1 as ramon, CARGO_TARGET_DIR=~/src/fxdax-harness-target: test-e2e.sh built and ran its harness tests and fxfs-guest-checks-dax-delay's VM, the runner, guest agent, extentd, fxdaxctl, fxdax-gc and Firecracker (musl, real seccomp) from that directory; the worktree's target/ held only Firecracker's source copy (target/firecracker, which must be in the repo for its config), firecracker-series' copy and the UCD zip. FXDAX_E2E_BUILD_ONLY copied the same artifacts. Blind spot: nothing ran test-e2e.sh with CARGO_TARGET_DIR; GCE hosts used the default. The bench/A-B drivers on ubu1 now can use it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
test-e2e.sh resolves each workspace's target directory with cargo metadata (CARGO_TARGET_DIR honoured) and takes every binary from it; verified on ubu1 with a separate target dir.
<!-- SECTION:FINAL_SUMMARY:END -->
