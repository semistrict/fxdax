---
id: TASK-269
title: >-
  Firecracker series strictly opt-in: linear, tagged, everything fxdax needs
  behind the `fxdax` feature
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 22:31'
labels:
  - firecracker
dependencies: []
priority: high
type: task
ordinal: 697000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A downstream user (LangSmith sandboxes) ports the Firecracker series (vendor/firecracker, semistrict/firecracker fxdax-v1.16.1, 58 commits on v1.16.1) onto their own fork, whose default build must not change. Today the series breaks any build outside fxdax's checkout (vmm's optional path dependency on ../../../../crates/fxdax, which Cargo resolves even unused), carries fxdax's ~4k-line dependency graph in its Cargo.lock, and changes default builds: ~23 VMM-thread seccomp rules in resources/seccomp, frame pointers in .cargo/config.toml, PUT /extent and the swagger, VM config and snapshot-load fields, MMIO shared memory regions and a u32 device type, signal handling, KvmMemSlot and pmem, vCPU EFAULT logging and exit codes 158/159, a vsock log. TASK-63 asked part of this before and was dropped; the user asked for it now.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A new linear branch on v1.16.1 (no merges), tagged, every commit building with and without --features fxdax (a script in fxdax's repo checks it)
- [ ] #2 Without the feature: git diff v1.16.1 -- resources/seccomp src/firecracker/swagger empty; the default build matches upstream but for the general fixes, which are commits of their own; upstream's unit tests pass on it
- [ ] #3 The series builds outside fxdax's checkout: no fxdax dependency or fxdax lock in it; fxdax's build injects the dependency, lock, frame pointers and feature
- [ ] #4 With the feature the tree is functionally what 06c161526 is; each difference explained
- [ ] #5 fxdax's scripts, CI and the seccomp test build Firecracker the new way; vendor/firecracker at the new tag; CI green; guest suites on GCE (release and debug) pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
