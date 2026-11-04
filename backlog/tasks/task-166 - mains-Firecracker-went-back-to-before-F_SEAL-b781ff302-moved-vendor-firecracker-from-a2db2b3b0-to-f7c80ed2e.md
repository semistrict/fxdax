---
id: TASK-166
title: >-
  main's Firecracker went back to before F_SEAL: b781ff302 moved
  vendor/firecracker from a2db2b3b0 to f7c80ed2e
status: Done
assignee: []
created_date: '2026-11-04 08:36'
updated_date: '2026-11-04 10:39'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 619000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
cde62f9ab moved main's vendor/firecracker to a2db2b3b0 (VIRTIO_EXTENT_F_SEAL, TASK-153.15: the daemon asks the guest to seal). b781ff302, an unrelated extent_remote commit, moved it back to f7c80ed2e, a submodule left behind in a worktree committed with the rest, so main's VMM never asks a guest to seal. Found 2026-11-04 by TASK-164.1's agent. Blind spot: CI builds no Firecracker, and the guest suites pass without seal asks; nothing checks the pointer only moves forward.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 main's vendor/firecracker is a2db2b3b0 or after
- [x] #2 CI refuses a push or pull request whose vendor/firecracker isn't ahead of its base's (scripts/firecracker-forward.sh), shown red on b781ff302
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
main's vendor/firecracker is a2db2b3b0 again (F_SEAL), and CI's firecracker-forward job refuses a pointer that isn't ahead of its base's (scripts/firecracker-forward.sh, tested, red on b781ff302). Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
