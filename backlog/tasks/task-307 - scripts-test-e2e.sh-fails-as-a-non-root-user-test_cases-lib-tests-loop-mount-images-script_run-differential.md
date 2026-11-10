---
id: TASK-307
title: >-
  scripts/test-e2e.sh fails as a non-root user: test_cases' lib tests loop-mount
  images (script_run, differential)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 13:06'
updated_date: '2026-11-10 09:30'
labels:
  - bug
  - scripts
dependencies: []
priority: medium
ordinal: 716000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-09, main 027a3926, user ramon in kvm and docker groups, no sudo), scripts/test-e2e.sh fxfs-guest-checks-dax-delay stopped before any case: its first step, cargo test -p test_cases --features guest, failed 19 of 114: script_run::tests::* and differential::tests::* mount images ('mount: .../mounted: failed to setup loop device for .../image'), which only root can. The GCE hosts ran as root. The tests that need root should say so (skip with a reason, or a suite of their own run as root), so that a non-root host runs the rest; FXDAX_E2E_BUILD_ONLY/PREBUILT skips them all, the VMM's firecracker part too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 scripts/test-e2e.sh runs as a non-root user on a KVM host, its root-only harness tests skipped and said so
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. The 19 tests that make a script_run::tests::Loop (loop-mounted ext4/XFS) can't run unprivileged: a user namespace mounts neither, and FUSE's (fuse2fs) aren't the kernel's, which they hold the model to. Mark them #[ignore = "needs root, ..."]; Loop asserts euid 0, saying to ignore a new test that makes one.
2. scripts/test.sh's guest part: as root, nextest --run-ignored all (CI runs it as root, as before); otherwise say on stderr that the root-only tests are left out.
3. test-e2e.sh runs the harness's tests as test.sh's guest part (FXDAX_TEST_PARTS="guest firecracker"), via nextest, as ubu1's setup note already describes, instead of its own cargo test of one feature.
4. Verify on ubu1 as ramon: test.sh guest part passes with the 19 skipped and said so; as root (sudo) they run and pass; test-e2e.sh as ramon gets past its harness tests and runs a case.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Decision (OPEN_QUESTIONS.md): no unprivileged substitute for the 19 Loop tests; ignored with the reason, run as root via --run-ignored all. Found while running: the firecracker part's two persistence tests (device_manager::{pci_mngr,persist}::test_device_manager_persistence) open tap 'hostname', EPERM as a user. A persistent tap owned by the user can be opened unprivileged, so test.sh runs them as root or where /sys/class/net/hostname/owner is the user, else leaves them out and says so; ubu1 got ~/docs/system-setup.d/144-fxdax_vmm_test_tap.sh (systemd unit making that tap, ramon's). Verified on ubu1 as ramon: guest part 117 + 95 (19 skipped, message) + 8; as root 114/114; firecracker part 30 + 31 with the tap, 30 + 29 and the message without it; full test-e2e.sh as ramon ran its harness tests and booted fxfs-guest-checks-dax-delay, which failed only append_writeback's budget (5.36 s of 3 s at load ~50, TASK-263, noted there). Blind spot: every host ran this as root (GCE, CI's guest job), so nothing showed which tests need it; now a test that makes a Loop without the ignore fails saying so. Structure: one switch per part in test.sh owns the root rule; test-e2e.sh runs the guest part instead of its own cargo test.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
test-e2e.sh and test.sh's guest and firecracker parts run as a user: root-only tests (19 loop-mount, 2 tap) are left out with a message, run as root (or with the user's tap) otherwise. Verified on ubu1 as ramon and as root.
<!-- SECTION:FINAL_SUMMARY:END -->
