---
id: TASK-166
title: >-
  main's Firecracker went back to before F_SEAL: b781ff302 moved
  vendor/firecracker from a2db2b3b0 to f7c80ed2e
status: To Do
assignee: []
created_date: '2026-11-04 08:36'
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
- [ ] #1 main's vendor/firecracker is a2db2b3b0 or after
- [ ] #2 CI refuses a push or pull request whose vendor/firecracker isn't ahead of its base's (scripts/firecracker-forward.sh), shown red on b781ff302
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
