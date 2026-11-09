---
id: TASK-269
title: >-
  Firecracker series strictly opt-in: linear, tagged, everything fxdax needs
  behind the `fxdax` feature
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 22:31'
updated_date: '2026-11-09 02:25'
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
- [x] #1 A new linear branch on v1.16.1 (no merges), tagged, every commit building with and without --features fxdax (a script in fxdax's repo checks it)
- [x] #2 Without the feature: git diff v1.16.1 -- resources/seccomp src/firecracker/swagger empty; the default build matches upstream but for the general fixes, which are commits of their own; upstream's unit tests pass on it
- [x] #3 The series builds outside fxdax's checkout: no fxdax dependency or fxdax lock in it; fxdax's build injects the dependency, lock, frame pointers and feature
- [x] #4 With the feature the tree is functionally what 06c161526 is; each difference explained
- [x] #5 fxdax's scripts, CI and the seccomp test build Firecracker the new way; vendor/firecracker at the new tag; CI green; guest suites on GCE (release and debug) pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New linear branch fxdax-v1.16.1-optin on v1.16.1: general fixes first (vCPU exit order, features page, alternate signal stack), then the fxdax feature and everything gated by it. 2. fxdax crate dependency and lock supplied by fxdax's build in a copy of the tree (scripts/firecracker-cargo.sh, vendor/firecracker.lock); frame pointers from fxdax's .cargo/config.toml. 3. Seccomp additions in resources/fxdax/seccomp, merged by build.rs; swagger additions in resources/fxdax/swagger.yaml. 4. Per-commit both-ways check script plus CI job; forward check accepts a listed rewrite. 5. Verify: per-commit (arm64, x86-64, fresh clone), BPF/JSON identity, vmm suites on GCE KVM both ways, guest rounds release+debug, main CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Series: semistrict/firecracker fxdax-v1.16.1-optin, tag fxdax-v1.16.1-r1 = 8e14cd916 (10 commits on v1.16.1 2038188f1; upstream v1.16.1 tag pushed to the fork). PCI stack (TASK-267) replayed on it: 517d438c5.
Verification: per-commit clippy -D warnings both ways on arm64 and x86-64 Linux (default build outside the checkout, --locked on the series lock), and from a fresh tagless clone as CI does; CI firecracker-series green over 15 commits (run 42549101699). Seccomp: aarch64 BPF byte-identical to 06c161526, merged JSON rule-for-rule on both arches; fxdax lock = old Cargo.lock. GCE KVM (n2, --test-threads=1): default 897/898, fxdax 923/924, only test_set_tsc, red on upstream v1.16.1 too (TASK-271). Guest rounds at r1 before the features-page test edits: release jailed 16/16 cases, debug 14/16 (TASK-257 flake); both workspace 2413/2414 (TASK-272, since fixed by 527e42a34). Main CI green at 36dbbe2e9 (42549101699). Also fixed the fxfs-posix mutant patch 527e42a34 left not applying.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Rewrote fxdax's Firecracker series as a strictly opt-in, linear series on v1.16.1 (fxdax-v1.16.1-optin, tag fxdax-v1.16.1-r1): three standalone general fixes, then everything fxdax needs behind the `fxdax` feature, with no fxdax dependency or lock in the series; fxdax's build injects them (scripts/firecracker-cargo.sh). Without the feature, resources/seccomp, the swagger and .cargo are upstream's and upstream's tests pass but test_set_tsc (red upstream too); with it, functionally 06c161526 (seccomp BPF identical). Verified per commit both ways on arm64, x86-64 and a fresh clone, on GCE KVM, guest rounds release and debug, and main CI green (42549101699).
<!-- SECTION:FINAL_SUMMARY:END -->
