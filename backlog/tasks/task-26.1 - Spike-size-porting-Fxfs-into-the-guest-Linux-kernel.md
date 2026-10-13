---
id: TASK-26.1
title: 'Spike: size porting Fxfs into the guest Linux kernel'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-13 19:53'
updated_date: '2026-10-13 21:32'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-3 -
    TASK-26.1-spike-porting-Fxfs-into-the-guests-Linux-kernel.md
parent_task_id: TASK-26
type: spike
ordinal: 37000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs is async Rust written for Fuchsia, with dependencies on fuchsia-async, std, and Fuchsia libraries, and the design requires its code to run in the guest kernel (doc-2, requirement 5). Rust-for-Linux's support for filesystems (VFS bindings) and for async is limited. Before committing to milestone 1, find out what the port takes and whether it is feasible.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A report in the backlog covers: the kernel version to target and its Rust and VFS bindings; how Fxfs's async code runs in the kernel; allocation; which Fuchsia dependencies need stubbing or porting, with sizes; and an estimate with a go/no-go recommendation
- [x] #2 A minimal Rust filesystem module builds against the chosen kernel and mounts in a libkrun guest
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Survey: kernel 7.3 Rust support (no VFS, virtio or async bindings upstream); libkrunfw's 6.12 config (no CONFIG_RUST, no modules); Fxfs's closure, std and Fuchsia API surface.
2. Measure Fxfs's stack use on one thread, optimized, with an example program (crates/fxdax/examples/fxfs-stack.rs), since kernel stacks are 16 KiB; record it in docs/measurements.
3. Time-boxed: a std for the kernel (guest/rust/std) in a custom sysroot, and a probe building Fxfs's third-party crates against it (guest/std-probe, scripts/probe-guest-std.sh); count what fails and why.
4. Prototype (AC2): a cargo-built no_std Rust staticlib (kvmalloc GlobalAlloc, printk logger, panic handler, block_on, a workqueue executor and hrtimers) linked with C VFS glue, built into libkrunfw v5.6.2's kernel (scripts/build-guest-kernel.sh); libkrun's fxfs-guest test boots it with krun_set_kernel, mounts it and reads a file served by async Rust (scripts/test-guest-kernel.sh).
5. Write the report as backlog doc-3: kernel choice, build approach, async, allocation, stack, dependencies with sizes, estimate, go/no-go.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Report: doc-3. Recommendation: go; porting Fxfs (without the extent-store device) is ~9-14 engineer-weeks.
Prototype: guest/linux/fs/fxfs (C VFS glue and runtime), guest/rust (linux-rt runtime, fxfs-linux stand-in, a kernel std), built by cargo outside Kbuild and linked into libkrunfw v5.6.2's 6.12.109 kernel (scripts/build-guest-kernel.sh). libkrun's fxfs-guest test boots it with krun_set_kernel, mounts fxfs, lists and reads a file whose contents a spawned task returns after an hrtimer: passes on macOS arm64 (scripts/test-guest-kernel.sh). x86-64 kernel builds with the Rust linked, not yet booted.
Stack: Fxfs's metadata operations take 2-7 KiB, but mount with replay 34 KiB, format 34-35, journal compaction 25-28 (crates/fxdax/examples/fxfs-stack.rs, docs/measurements/fxfs-stack-2026-10-13): the main risk; plan is 32 KiB guest stacks plus boxing the largest futures.
Dependencies: against guest/rust/std as the sysroot's std, 44 of 61 third-party crates build unchanged (scripts/probe-guest-std.sh); 7 fail, for hash maps called by path, per-thread state, std float math, and small gaps.

Validation: scripts/test.sh 1860 passed (macOS); scripts/test-linux.sh 1864 passed (Lima); scripts/upstream-changes.py --check passes; scripts/test-guest-kernel.sh: fxfs-guest OK on macOS arm64 after a clean kernel rebuild with no kernel warnings from fs/fxfs; x86-64 kernel cross-builds (Rust object 14 KB); clippy clean for guest/rust on both targets. No upstream files changed, so no new deviations.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Sized porting Fxfs into the guest's Linux kernel and recommended going ahead (doc-3): ~9-14 engineer-weeks without the extent-store device. Built the base: Rust built by cargo outside Kbuild (guest/rust: runtime, executor, kernel std) and linked into libkrunfw's 6.12 kernel with C VFS glue (guest/linux/fs/fxfs); libkrun's fxfs-guest test boots it, mounts fxfs and reads a file a spawned task produces after a timer, passing on macOS arm64. Found the main risk, stack: mount/replay and compaction take 25-34 KiB against 16 KiB kernel stacks (docs/measurements/fxfs-stack-2026-10-13). 44 of 61 third-party crates build unchanged against the kernel std; the 7 that fail are classified. Verified with the guest test, both workspace test suites and the upstream check.
<!-- SECTION:FINAL_SUMMARY:END -->
