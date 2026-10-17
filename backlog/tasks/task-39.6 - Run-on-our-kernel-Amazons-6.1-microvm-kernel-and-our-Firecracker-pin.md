---
id: TASK-39.6
title: Run on Amazon's 6.18 microvm kernel and our Firecracker pin
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-17 22:01'
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
- [x] #1 fxdax's Firecracker changes are a commit series on top of v1.16.1 (the main system's pin), rebased cleanly, and the guest suites pass with it
- [x] #2 Fxfs and the virtio_extent driver build into Amazon's 6.18 kernel as a patch series (git format-patch, with the Rust object's build), and the guest suites pass on it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. scripts/build-guest-kernel.sh builds Amazon's microvm-kernel-6.18.25-57.115 with Firecracker's 6.18 CI config (FXDAX_GUEST_KERNEL_BASE selects it; libkrunfw's 6.12 stays the default until it passes), with fxdax's patches, fs/fxfs, virtio_extent and the Rust object.
2. Port guest/linux's C (fs/fxfs, virtio_extent, the task patches) to what 6.18 changed.
3. The guest suites, pjdfstest and a bench pass on it in Lima (arm64) and on GCE (x86-64); then it becomes the default, and libkrunfw goes.
4. fxdax's Firecracker changes as a series on v1.16.1: separate step.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decided with the user 2026-10-17: a backport patch series of fs/fxfs, its Rust and virtio_extent onto Amazon's 6.1 microvm kernel (not shipping fxdax's 6.12 kernel).

2026-10-17: retargeted from 6.1 to Amazon's 6.18 microvm kernel, as the user allowed if 6.1 gave trouble. Upstream Firecracker's kernel policy (docs/kernel-policy.md, main e786ba84f): guest 6.1 supported from v1.9.0 with a minimum end of support of 2026-09-02, already past; guest 6.18 from v1.16.1 (the main system's pin) until at least 2028-06-01. amazonlinux/linux tags microvm-kernel-6.18.25-57.115.amzn2023; Firecracker's CI config resources/guest_configs/microvm-kernel-ci-<arch>-6.18.config. fxdax's kernel is libkrunfw's 6.12: a forward port to 6.18, not a backport to 6.1.

2026-10-17 progress:
- Kernel: Amazon Linux kernel6.18-6.18.54-121.166.amzn2023 (the ordinary 6.18 release Firecracker's rebuild.sh picks; the microvm-kernel6.18 tags lag), with Firecracker's 6.18 CI configs vendored in guest/linux/configs (v1.16.1 predates them: they landed on main in July).
- Ports to 6.18: hrtimer_setup; pfn_t gone (dax_direct_access and dax_iomap_fault take unsigned long pfns); write_begin/write_end take a kiocb; mkdir returns a dentry; dax_break_layout and dax_break_layout_final (DAX pages are idle at refcount 0 now; truncate_inode_pages_final warns on DAX entries left).
- DAX bug that 6.18 turned into SIGBUS: a write(2) consumed a page's relocation mark, and a racing fault then kept the stale DAX entry (6.12 warned once and retried). Fixed: only faults forget the mark (commit b697116).
- Harness: the guest agent's stdout framed on the console, the kernel's lines to stderr, loglevel=5 (commit 88ca964).
- Firecracker: the 8 fxdax commits rebased onto v1.16.1 as branch fxdax-v1.16.1 of semistrict/firecracker; every commit builds with and without the fxdax feature, clippy clean. v1.16.1 API differences: VirtioDevice::reset returns Option<(interrupt, events)>, read_config has no default, vm-memory's GuestMemory (not GuestMemoryBackend), DescriptorChain not Send (a Request wrapper), attach_virtio_device.
- Verified on arm64 (Lima): all 9 guest suites on 6.18 with both Firecrackers.

Validation: arm64 (Lima) all 9 guest suites + pjdfstest (with and without DAX) on 6.18; x86-64 (GCE n2-standard-8) 1987 workspace tests and all 11 guest suites incl. pjdfstest; macOS 1975 tests; upstream-changes.py --check passes; the Firecracker series checked commit by commit, with and without fxdax, clippy clean. Remaining: AC 'patch series': the kernel changes are applied by scripts/build-guest-kernel.sh (rsync of fs/fxfs and drivers/virtio_extent, two .patch files, Kconfig/Makefile edits), not yet exported as a git format-patch series the main system can apply.

Patch series: guest/linux/patches (4 commits), built by scripts/build-guest-kernel.sh as commits in the kernel tree; out/<arch>/{patches,libfxfs_linux.a,fxdax.config} build it elsewhere. git am on a fresh checkout reproduces the tree; guest suites pass on the series-built arm64 kernel; x86-64 builds, the same series.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The guest kernel is Amazon Linux's 6.18 (kernel6.18-6.18.54-121.166.amzn2023; Firecracker supports 6.18 guests from v1.16.1, and 6.1's support had ended), configured with Firecracker's 6.18 CI configs plus fxdax.config, with fxdax's changes a 4-patch series (guest/linux/patches) that the build applies and exports. fs/fxfs and virtio_extent were ported to 6.18 (hrtimer_setup, no pfn_t, kiocb write_begin/end, mkdir returning a dentry, dax_break_layout), and a DAX race that 6.18 turned into SIGBUS fixed. Firecracker's fxdax changes are 8 commits on v1.16.1 (semistrict/firecracker fxdax-v1.16.1), each building with and without the feature. Verified: arm64 Lima all guest suites + pjdfstest; GCE x86-64 1987 workspace tests + all 11 guest suites incl. pjdfstest; macOS 1975 tests; pgbench measured (TASK-41 follows).
<!-- SECTION:FINAL_SUMMARY:END -->
