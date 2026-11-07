---
id: TASK-149.38
title: 'pjdfstest on a FUSE mount of a store, in CI'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 05:20'
updated_date: '2026-11-07 11:17'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 661000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
pjdfstest checks POSIX semantics (permissions, ownership, setgid, sticky directories, times, errnos) and runs against the guest's Fxfs, but only in the kernel and nightly suites, and the nightly has never run (OPEN_QUESTIONS: no GCP credentials), so in practice only when someone runs scripts/test-guest-kernel.sh; and only in directories it makes, never on a filesystem's root. A FUSE mount of a store (TASK-222) runs it on the host's Node layer and fxfs-posix in a CI job in minutes. pjdfstest is BSD-2-Clause; the guest's case pins https://github.com/pjd/pjdfstest at 85a8aea9e685999ef0540392fd80535f873d7ff7. It needs root for chown tests: CI's runner has sudo.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A CI job builds pjdfstest at the guest case's pinned commit and runs it as root on a FUSE mount of a new store, failing on any test failure but those listed as known, each with a task
- [x] #2 A run also starts from the mount's root, chowned and chmodded first
- [x] #3 Failures it finds are fixed or listed with tasks
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax-mount --new CAPACITY: formats a new store at STORE, then mounts it (a scratch mount; CI's pjdfstest).
2. scripts/pjdfstest-fuse.sh: builds pjdfstest at the guest case's pin (one place for the pin, read by test-linux-host.sh, test-guest-kernel.sh and it), mounts a new store as root, runs prove in a directory of the mount, then on a fresh store from the mount's root after chown/chmod of it; fails on any failure not in scripts/pjdfstest-fuse-known.txt (each line a test and its task).
3. A CI job pjdfstest-fuse in ci.yml running it as root; scripts/tests test that the pins agree and the job runs the script.
4. Run in Lima as root; fix what it finds or list it with a task.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: fxdax-mount --new CAPACITY (d0d5bef42); scripts/pjdfstest-fuse.py (builds pjdfstest at the pin in FXDAX_PJDFSTEST_WORK, default target/pjdfstest; two runs on fxdax-mount --allow-other --new stores: in a directory of the mount, and from the root after chown to 65534 and back to 0, each chown's mode checked 0755, then chmod 0755; parses prove's summary into FILE:N and whole-file failures; fails on any not in scripts/pjdfstest-fuse-known, or a listed one passing in neither run); scripts/lib/pjdfstest.sh the pin's one place (test-guest-kernel.sh and test-linux-host.sh source it); CI job pjdfstest-fuse (debug build of fxdax-mount, sudo); scripts/tests/test_pjdfstest_fuse.py. --allow-other: pjdfstest's tests become other users with real setuid, and a FUSE mount without allow_other refuses every other user (the first Lima run: 3,214 failures, nearly all that).

Validation: Lima (aarch64 Ubuntu 24.04, kernel 6.8), root: in-a-directory Files=238 Tests=8798 PASS, from-the-root Files=238 Tests=8798 PASS, about 170 s each; script exit 0. The known list is empty. These runs were on the code after TASK-149.37's fixes; pjdfstest was not run against the code before them, so whether it would have found any of them is not measured. scripts tests 170/170 on macOS (6 new). The CI job itself runs on push: not run yet.

Landing: CI 42356882950's pjdfstest-fuse job green (both runs, in a directory and from the chowned and chmodded root); GCE run-20261107T093421Z workspace 2312/2312.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
CI job pjdfstest-fuse runs scripts/pjdfstest-fuse.py as root: pjdfstest at the guest case's pin (one place now, scripts/lib/pjdfstest.sh) on fxdax-mount --allow-other --new stores, in a directory and from the root chowned and chmodded, failing on anything not in scripts/pjdfstest-fuse-known (empty). Verified: 8,798 of 8,798 pass in both runs in Lima as root and in CI 42356882950's job; script tests pass.
<!-- SECTION:FINAL_SUMMARY:END -->
