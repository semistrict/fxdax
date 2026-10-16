---
id: TASK-27.3
title: 'fxdax: guest tests and fs bench under Firecracker'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 23:42'
updated_date: '2026-10-16 01:41'
labels: []
dependencies: []
parent_task_id: TASK-27
priority: high
type: feature
ordinal: 78000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
libkrun's test harness runs the guest suites and the fs bench with a virtio-fs root holding the test binaries; Firecracker has no virtio-fs. fxdax needs a harness that builds Firecracker, a root filesystem image with the guest test binaries, boots each case and reads its result, on GCE, as scripts/test-gce.sh and bench-gce.sh do for libkrun.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts run the guest suites and pjdfstest-fxfs-guest(-dax) under Firecracker on a GCE host, reporting each case's result
- [x] #2 The fs bench's fxfs-guest-dax and ext4-dax-pmem cases run under Firecracker and their comparison is saved in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. libkrun tests: test_cases::vm::Vm abstraction (libkrun or Firecracker per FXDAX_VMM); fxdax tests use it.
2. Firecracker backend: ext4 image of the root (mkfs.ext4 -d), config JSON, exec firecracker --no-api; guest agent as PID 1 (guest_init) mounts, raw console, resets the VM.
3. Guest kernel: 8250 serial console (+OF platform on arm64).
4. scripts: test-e2e.sh FXDAX_VMM=firecracker builds Firecracker; GCE scripts ship vendor/firecracker and pass FXDAX_VMM.
5. Verify: Lima (arm64) guest suites, then GCE x86 guest suites, pjdfstest, fs bench saved under docs/measurements.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
test_cases::vm::Vm (libkrun | Firecracker via FXDAX_VMM); guest agent as PID 1 (guest_init: mounts, raw console, stderr framed SO..SI on the console, sync + reset). Host spawns Firecracker (PDEATHSIG), console to console.txt (Firecracker drops serial bytes on a full nonblocking pipe), splits stderr. Lima arm64: extent-guest, fxfs-guest, -format, -crash, -dax, -crash-dax, -checks, -checks-dax all pass under Firecracker and libkrun. Early intermittent EIO from the daemon's writable MAP seen twice (not reproduced in 16 runs since); daemon now warns with the failing step.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
libkrun tests (vendor/libkrun 3cbeed8, b65d5ef): test_cases::vm::Vm boots libkrun or Firecracker (FXDAX_VMM=firecracker); guest agent as init under Firecracker; scripts ship and build Firecracker. Verified on GCE x86-64 under Firecracker: extent-guest, fxfs-guest, -format, -crash, -dax, -crash-dax, -checks, -checks-dax pass; pjdfstest-fxfs-guest and -dax PASS (238 files, 8798 tests each); libkrun regression of the same suite passes; fs bench saved in docs/measurements/gce-fs-bench-2026-10-16T0121Z (fxfs-guest-dax vs ext4-dax-pmem worst 1.57x lookup-missing).
<!-- SECTION:FINAL_SUMMARY:END -->
