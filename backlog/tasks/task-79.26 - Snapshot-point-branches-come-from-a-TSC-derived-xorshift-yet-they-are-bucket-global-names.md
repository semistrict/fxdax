---
id: TASK-79.26
title: >-
  Snapshot point branches come from a TSC-derived xorshift, yet they are
  bucket-global names
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 214000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
rand_branch has about 38 bits of entropy, from rdtsc. A collision between hosts in a shared bucket breaks the second VM's uploads (record_fork returns Corrupt), and a restore can then serve one tenant's filesystem to another.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:511, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:447, crates/fxdax/src/extentd/stores.rs:415.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-39.1. No task covers the TSC-xorshift rand_branch. TASK-39.1 (Done) built snapshot points forked into new branches.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
