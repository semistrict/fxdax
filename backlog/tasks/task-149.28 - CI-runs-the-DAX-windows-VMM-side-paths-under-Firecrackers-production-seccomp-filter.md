---
id: TASK-149.28
title: >-
  CI runs the DAX window's VMM-side paths under Firecracker's production seccomp
  filter
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 03:49'
updated_date: '2026-11-05 05:40'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 623000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The jailed VMM runs Firecracker's production seccomp filter (vendor/firecracker/resources/seccomp/{x86_64,aarch64}-unknown-linux-musl.json, section "vmm"), and fxdax's client and DAX window code (crates/fxdax/src/dax/uffd.rs, extentd/client.rs) runs on the VMM's threads. Four system calls copy-on-write DAX needed were missing from the filter: memfd_create, fallocate, ioctl UFFDIO_COPY (TASK-164.1) and pread64 (GCE r552). Each was found only on GCE, as the VM was killed with "bad syscall (N)": CI never runs this code under the filter. The blind spot: the filter is a list kept by hand beside code that changes, and only a jailed GCE run checked one against the other.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A Linux test CI runs (the workspace part of scripts/test.sh) compiles the "vmm" section of the JSON with vendor/firecracker's seccompiler, as Firecracker's build does, and runs the VMM side of a copy-on-write window under it: the window created, extents mapped, pages made private with and without a source page, unmapped, released
- [x] #2 A system call the filter refuses fails the test with its name and number, its arguments and its thread, every one the run makes, rather than killing the test process
- [x] #3 Shown red: with pread64 taken out of the JSON the test fails naming pread64 (17 on x86-64); green with it back
- [x] #4 The macOS build and FXDAX_TEST_PARTS=workspace scripts/test.sh still pass; CI installs what the test needs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/fxdax/tests/seccomp.rs (Linux, its own binary): compile Firecracker's JSON with vendor/firecracker's seccompiler (Linux dev-dependency; workspace excludes vendor/), split output, raw "vmm" and "vcpu" programs; rewrite their RET TRAP to RET USER_NOTIF.
2. Confine threads with SECCOMP_FILTER_FLAG_NEW_LISTENER; a supervisor per thread, started before it is confined, records each notified call (number, arguments, thread) without allocating and answers SECCOMP_USER_NOTIF_FLAG_CONTINUE, so a run lists every refusal.
3. fxdax::thread::set_spawner with a spawner confining each thread as it starts, as Firecracker's confine_extent_threads does: the client's receiving thread and the DAX fault thread; the device's work (activation, requests, reset) on threads run through it.
4. Scenario after extentd/tests.rs windows_copy_on_write_in_the_vm_process: daemon in-process unconfined; create a copy-on-write window; map sealed and zeros; loads; stores making private pages with and without a source; a worker's write restricted, a vCPU (under "vcpu") calling guest_access_failed and describe_windows; UNMAP, PRIVATE; drop (release). Join supervisors; assert nothing refused, naming syscalls through libseccomp.
5. glibc vs musl: a short table of glibc's calls whose musl counterpart the JSON allows unconditionally (x86-64 openat for open), from what the run shows.
6. CI: checkout with submodules, libseccomp-dev. README notes the workspace needs the submodule.
7. Red: pread64 out of the JSON, the test names it; green with it back. macOS FXDAX_TEST_PARTS=workspace scripts/test.sh.

Revised in step 3: the spawner starts threads from a thread of its own, never confined, as Firecracker's does. A thread started by a confined thread inherits its filter and listener, and can have no listener of its own.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04, first Linux runs (OrbStack, aarch64): the test compiled and hung past 120 s. Why: the spawner started threads from the thread that asked, and the DAX fault thread is asked for from fc_extent_activate, which is confined. A thread a confined thread starts inherits its filter, listener and all, and a thread's filters can hold only one listener, so its own confinement failed (EBUSY). The fault thread panicked, and the guest's first load waited for ever. Fixed (not yet run): the spawner starts threads from a thread of its own that is never confined, as Firecracker's confine_extent_threads does. Then the host's disk filled (119 MiB free on /System/Volumes/Data) and OrbStack's daemon died, so verification stopped there. Nothing is committed: the change has not passed a run.

Runs after the spawner fix (OrbStack, aarch64, libseccomp 2.6.0):
- First full run: 13 refusals, none a production call. fcntl(fd, F_GETFD) on every confined thread is Rust std's check that an OwnedFd being dropped is still open (debug_assert_fd_is_open, gated by core::ub_checks, so only in builds with debug assertions; Firecracker's release build has none, and its debug build runs an empty filter). statx(fd, "", AT_EMPTY_PATH) is std's File::metadata on glibc; on musl std calls fstat, which both JSONs allow with any arguments. in_production() maps these, with x86-64 openat to musl's open. Nothing in fxdax or the device calls F_GETFD itself (rg).
- Green: 1 test passed, 2.9 s.
- Red: pread64 taken out of the aarch64 JSON: "vmm's refused extentd client pread64 (67) of 0x18, …" (3 calls). With memfd_create(MFD_CLOEXEC), fallocate and ioctl UFFDIO_COPY taken out instead, all three are named: memfd_create (279) on fc_extent_activate, fallocate (47) twice and ioctl (29) 0xc028aa03 4 times on extentd client. Green again with the JSON restored (the submodule is clean).
- Ubuntu 24.04's libseccomp 2.5.5, which CI installs, compiles both JSONs with seccompiler-bin (x86_64 4,413 bytes, aarch64 3,378).
- Linux clippy part (scripts/test.sh): clean.
Not run: x86-64. OrbStack here is arm64, and a filter for x86-64 can't run under Rosetta, so CI's first run is the first x86-64 one. glibc may make other calls there that musl makes differently, which in_production would then need.
Structure: the filter is a hand-kept list beside code that changes. This test is what keeps the two in step on every CI run. The remaining gap is calls only musl makes, and threads starting.

macOS, FXDAX_TEST_PARTS=workspace scripts/test.sh: 2,053 of 2,053 passed (584 s). AC 1 and AC 4 wait for CI's first run, the first on x86-64 and the first with the submodule checkout and libseccomp-dev.

x86-64: CI 41969564320 on seccomp-guard, workspace part: fxdax::seccomp copy_on_write_windows_run_under_firecrackers_seccomp_filters passed. macOS workspace 2068/2068 on the landing tree. Red runs were on aarch64 (pread64 is 67 there); x86-64's red wasn't run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
crates/fxdax/tests/seccomp.rs runs the VMM side of a copy-on-write window (map, private with and without a source, fill, punch, unmap, release) on threads under the vmm and vcpu filters compiled from vendor/firecracker's JSON, refusals notified and named with arguments and thread. Red on aarch64 with each of pread64, memfd_create, fallocate, UFFDIO_COPY taken out; green with them; green on x86-64 in CI 41969564320.
<!-- SECTION:FINAL_SUMMARY:END -->
