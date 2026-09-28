---
id: TASK-9
title: libkrun DAX on Linux (KVM)
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 19:56'
labels: []
milestone: m-1
dependencies:
  - TASK-6
type: feature
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On KVM the DAX window is host memory registered as a guest memory slot; guest faults on pages the pager has not supplied must be caught (userfaultfd or equivalent) and routed to the kernel's pager, and write-protect faults must mark pages dirty.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A libkrun guest on Linux reads and writes a file through a DAX mapping, with data matching FUSE reads
- [x] #2 Write faults from the guest dirty pages in the kernel and writeback persists them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Decision (user): userfaultfd. arm64 KVM reports unresolvable faults on ordinary memslots only as EFAULT without an address, so faults must be caught in the memory KVM follows.
1. fxdax dax::uffd (Linux): UffdSecondStage over the memslot's host memory (libkrun's shm mapping). Unmapped pages are anonymous, registered for MISSING faults; mapped pages are the kernel's pages duplicated with mremap(old_size=0), RW, registered for WP, and write-protected while the kernel maps them read-only. A fault thread passes events to DaxWindow::fault and wakes the faulting thread.
2. The userfaultfd must handle kernel faults (KVM's): /dev/userfaultfd, else userfaultfd(2) without UFFD_USER_MODE_ONLY; a clear error otherwise.
3. Unresolved faults are fatal on both hosts, as for a Zircon guest (macOS: vCPU error instead of the MMIO fallback).
4. libkrun adapter (Linux): window over the shm region with the uffd second stage.
5. Tests: fxdax test driving the second stage with host-thread accesses (missing, write-protect, writeback re-protect); virtiofs-fxdax e2e in Lima; rerun macOS e2e.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Wire libkrun's fxdax adapter setupmapping/removemapping to fxdax::Vfs (TASK-6 API: create_dax_window with the shm size, setup_mapping, remove_mapping) once the shm region is the DaxWindow's host memory and guest faults reach DaxWindow::fault.

After TASK-8: the adapter creates the DAX window only on macOS (FxdaxFs::open). On Linux, KVM follows host mappings, so the memslot could be the window's host reservation (DaxWindow::host_base) with no SecondStage; guest accesses to unmapped window pages then fault in KVM (EFAULT/KVM_EXIT_MEMORY_FAULT) and must reach DaxWindow::fault. virtiofs-fxdax e2e test exists (TASK-8); run it in Lima.

- fxdax::UffdSecondStage (crates/fxdax/src/dax/uffd.rs) is the second stage in the memory KVM follows. Unmapped pages are anonymous memory registered MISSING. Mapped pages are mremap(old_size=0) duplicates of the kernel's shared page mappings, kept RW, registered WP, and write-protected while the kernel maps them read-only. A "dax faults" thread resolves events through DaxWindow::fault and wakes the faulting thread.
- Requires kernel-fault userfaultfd: /dev/userfaultfd (Lima: udev rule granting the kvm group, documented in README) or vm.unprivileged_userfaultfd=1.
- Unresolved faults are fatal on both hosts (Linux: abort; macOS: vCPU error), replacing the earlier macOS MMIO fallback.
- Bug found: the e2e runner links its own fxdax, and on Linux libkrun.so's zx_* calls bound to the runner's kernel (flat namespace), so handles were BAD_HANDLE (EBADF). Fixed: libkrun.so links with -Bsymbolic-functions when fxdax is enabled.
- The test runner counted TIMEOUT as neither pass nor fail (reported "OK - 0/0"). It now counts as a failure.
- Lima e2e: the host mount is read-only, so rsync the repo to ~/fxdax-test and run make/make test there (CC_LINUX=cc LIBCLANG_PATH=~/libclang EXTRA_LIBPATH_Linux=~/libkrunfw/lib/aarch64-linux-gnu).
- Known limit: each mapped page is its own VMA twice over (the kernel's host window plus the second stage). A fully populated large window can exceed vm.max_map_count (65530).
- Validation: virtiofs-fxdax e2e OK on Linux (KVM) and macOS; scripts/test.sh 1739 passed on both; kernel-cfg --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Linux KVM guests use fxdax DAX mappings. KVM follows the virtio-fs shm region's host memory, which a userfaultfd-backed second stage keeps in step with the DAX window: unmapped pages report missing faults, and clean or read-only pages are write-protected, so first touch goes through Fxfs's pager and first store through its dirty tracking. The virtiofs-fxdax e2e test passes on Linux and macOS. Also fixed: libkrun.so bound its Zircon syscalls to the host program's copy of the kernel on Linux, and the test runner hid timeouts.
<!-- SECTION:FINAL_SUMMARY:END -->
