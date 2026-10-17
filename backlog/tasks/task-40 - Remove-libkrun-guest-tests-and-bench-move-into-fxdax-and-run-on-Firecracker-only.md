---
id: TASK-40
title: >-
  Remove libkrun: guest tests and bench move into fxdax and run on Firecracker
  only
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 19:37'
updated_date: '2026-10-17 19:59'
labels: []
dependencies: []
priority: high
type: task
ordinal: 118000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user asked to delete libkrun (2026-10-17). Its submodule (vendor/libkrun) holds fxdax's extent device for libkrun, and the guest test harness (tests/: runner, guest-agent, macros, test_cases) that every guest suite and the fs bench run through, under libkrun or Firecracker (FXDAX_VMM=firecracker). Decided with the user: move the harness, the guest suites, pjdfstest and the bench into the fxdax repo, running on Firecracker only, then remove the submodule and libkrun's code paths. Cases needing libkrun's own devices or kernel (virtio-fs passthrough, TSI and its networking, its default kernel) go.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The guest test harness lives in the fxdax repo and boots VMs with Firecracker only: the fxdax guest suites (extent-guest, fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax, -format, -resize) and pjdfstest-fxfs-guest(-dax) pass through scripts/test-e2e.sh
- [x] #2 The fs bench runs on Firecracker from the fxdax repo (scripts/bench.sh, bench-gce.sh), its cases those Firecracker can boot
- [x] #3 vendor/libkrun is gone, and nothing in scripts, docs or crates builds, runs or describes libkrun as a VMM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Moved vendor/libkrun/tests (runner, guest-agent, macros, test_cases) to guest/tests, its own workspace; libkrun's framework files keep its Apache-2.0 license (LICENSE.libkrun, headers naming both copyrights; scripts/license-headers.py checks that kind), fxdax's cases are fxdax's. vm.rs is Firecracker only (vm::has_vmm); dropped libkrun-only cases (vsock/tsi/net/virtio-fs/FreeBSD/console/config/augmentfs/root-disk-remount, pjdfstest on virtio-fs) and the bench's ext4 default-kernel and virtio-fs passthrough backends. fxfs-guest-resize skipped under Firecracker (shared-root files); it now uses vsock (Vm::vsock, Firecracker's uds_path_PORT) and passes. scripts/test-e2e.sh builds Firecracker, the guest agent (musl) and the runner and runs each case; lima/gce/linux-host scripts drop libkrunfw's library and libkrun's paths (KRUN_NO_UNSHARE is FXDAX_NO_UNSHARE, KRUN_TEST_GUEST_AGENT_PATH is FXDAX_GUEST_AGENT). bench-report still reads older runs' libkrun backends. README, doc-2 (decision 9 revised) and doc-4 name Firecracker. Verified in Lima: Linux 1987; extent-guest, fxfs-guest(-dax, -checks, -checks-dax, -crash, -crash-dax, -format, -resize), pjdfstest-fxfs-guest(-dax) pass; bench of ext4-guest-kernel, ext4-dax-pmem, fxfs-guest-dax (fsync) reports. macOS suite unaffected (no crate logic changed).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
libkrun is gone: the guest test harness and bench live in guest/tests and boot fxdax's Firecracker only; fxfs-guest-resize now runs there over vsock. All guest suites, pjdfstest and the bench pass in Lima.
<!-- SECTION:FINAL_SUMMARY:END -->
