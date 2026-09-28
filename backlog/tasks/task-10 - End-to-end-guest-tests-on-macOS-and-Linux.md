---
id: TASK-10
title: End-to-end guest tests on macOS and Linux
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 23:13'
labels: []
milestone: m-2
dependencies:
  - TASK-8
  - TASK-9
type: task
ordinal: 10000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Unit tests exercise the server and kernel directly; only a real guest proves the whole stack. Tests boot a libkrun guest with an fxdax share and run filesystem workloads with and without DAX.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An e2e test boots a guest on macOS and on Linux, mounts the share, and runs create/read/write/rename/unlink/mmap workloads with DAX enabled and disabled
- [x] #2 Data written by the guest survives guest shutdown, host remount and fsck
- [x] #3 The tests run from the repo with one documented command per host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
vendor/libkrun branch fxdax still carries the RedoxFS e2e test (tests/test_cases/src/test_virtiofs_redoxfs.rs, and a redoxfs path dependency in tests/test_cases/Cargo.toml that does not resolve in the fxdax repo); port it to fxdax here.

TASK-8 ported the RedoxFS e2e test as virtiofs-fxdax (DAX on; mmap, FUSE/DAX coherence, namespace ops, truncate, eviction write-back, host fsck). Passes on macOS. Remaining: Linux run, DAX-disabled variant, documented one-command runs.

After TASK-9: virtiofs-fxdax also passes on Linux (Lima, KVM, userfaultfd). Remaining: a DAX-disabled variant, and one documented command per host (Lima needs an rsync copy because the host mount is read-only; see TASK-9 notes).

TASK-13: virtiofs-fxdax also passes on x86-64 KVM (GCE), with `scripts/test-gce.sh all` as the one command there.

- There are two cases, virtiofs-fxdax (dax=always, 512 MiB window) and virtiofs-fxdax-nodax (no window, dax=never). Both run the same workloads and assert the file's DAX attribute. After shutdown the host remounts and fscks the image before checking it (AC2).
- One command per host (README): scripts/test-e2e.sh (macOS HVF, native Linux KVM), scripts/test-e2e-linux.sh (Lima, rsync copy), scripts/test-gce.sh all (x86-64 GCE, runs test-e2e.sh on the host).
- Also fixed: empty-argument handling in the Lima scripts (printf ' %q' with no args passed ''), and bash 3.2 empty-array expansion on macOS.
- Validation: both cases pass on macOS, Lima (arm64 KVM) and GCE (x86-64 KVM). scripts/test.sh passes with 1739 tests on macOS and 1740 on Lima and GCE; kernel-cfg --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax's guest tests cover DAX on and off. They run the same workloads (create, read, write, rename, link, symlink, unlink, truncate, mmap, eviction write-back), and afterwards the host remounts and fscks the image and checks what the guest wrote. Each host has one documented command: scripts/test-e2e.sh, scripts/test-e2e-linux.sh (Lima) and scripts/test-gce.sh. Both cases pass on macOS HVF, arm64 KVM and x86-64 KVM.
<!-- SECTION:FINAL_SUMMARY:END -->
