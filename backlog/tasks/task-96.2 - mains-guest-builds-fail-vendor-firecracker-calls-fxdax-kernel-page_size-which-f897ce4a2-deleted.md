---
id: TASK-96.2
title: >-
  main's guest builds fail: vendor/firecracker calls fxdax::kernel::page_size,
  which f897ce4a2 deleted
status: Done
assignee: []
created_date: '2026-10-29 02:09'
updated_date: '2026-10-29 02:34'
labels:
  - bug
dependencies: []
parent_task_id: TASK-96
priority: high
ordinal: 464000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
f897ce4a2 (TASK-96.1, the Zircon port deleted) removed fxdax's kernel module; the pinned vendor/firecracker (12b14e485) calls fxdax::kernel::page_size() in src/vmm/src/devices/virtio/extent/device.rs:166, so vmm doesn't compile and every guest suite (scripts/test-gce.sh, test-e2e.sh) fails before it runs. fxdax::pages::page_size is its replacement. No commit on semistrict/firecracker's branches has the change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 vendor/firecracker's pin compiles against main's fxdax
- [x] #2 A guest suite runs on GCE from main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
semistrict/firecracker fxdax-v1.16.1 83d7d3cf0: the extent device takes its page size from fxdax::pages::page_size. fxdax main 79a4ab93e pins it. Verified on GCE with that pin: the guest kernel, Firecracker and the guest suites built and ran (fxfs-guest-dax, -checks-dax 2 rounds each; -checks-dax 15 rounds; the fxdax crate's tests).
<!-- SECTION:FINAL_SUMMARY:END -->
