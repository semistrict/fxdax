---
id: TASK-39.6
title: Run on our kernel (Amazon's 6.1 microvm kernel) and our Firecracker pin
status: To Do
assignee: []
created_date: '2026-10-17 19:16'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 116000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax builds its filesystem (fs/fxfs, Rust) and the virtio_extent driver into libkrunfw's 6.12 kernel (scripts/build-guest-kernel.sh). The main system ships Amazon's 6.1 microvm kernel, and pins Firecracker v1.16.1; fxdax's Firecracker fork (vendor/firecracker) is close to that pin but must become a commit series we can rebase.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fxfs and the virtio_extent driver build into Amazon's 6.1 microvm kernel as a patch series, and the guest suites pass on it
- [ ] #2 fxdax's Firecracker changes are a commit series on top of v1.16.1 (the main system's pin), rebased cleanly, and the guest suites pass with it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
