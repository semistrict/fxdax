---
id: TASK-15
title: 'Unmount occasionally hangs in FxVolume::terminate'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 00:31'
updated_date: '2026-10-10 14:44'
labels: []
dependencies: []
references:
  - crates/fxdax/src/filesystem.rs
  - crates/fxfs-platform/src/fuchsia/volume.rs
  - crates/fxfs-platform/src/fuchsia/volumes_directory.rs
  - vendor/fuchsia-kernel/src/storage/fxfs/platform/src/fuchsia/pager.rs
type: bug
ordinal: 15000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Rarely, closing or unmounting a filesystem never returns: Filesystem::unmount waits forever on the executor, and every thread in the process is idle (futex waits; tokio workers parked), so some future is never woken. It is pre-existing: it reproduces on df29131, before the public API rework (TASK-14), as well as after it. Found while validating TASK-14 on Linux.

What is known:
- Seen only on Linux (Lima, aarch64, 4 KiB pages). macOS has not been stress-tested, so it is not known to be Linux-only.
- Tests seen hanging: contents_survive_remount (three times, including once on df29131 as vfs::tests::contents_survive_remount) and readdir (once). Both hung in the final close, after files were created or looked up and read.
- Rate: about one hang per several hundred to two thousand rounds of the fxdax suite (45 tests) with cargo nextest run -p fxdax -j 24 --stress-duration 40m. Hung at rounds 590 and 20 (after TASK-14) and 196 (df29131); one 1909-round run did not hang. Once in a full scripts/test.sh run in Lima. Never in 240 runs of contents_survive_remount alone (40 serial, 200 eight-way parallel), so it seems to need load.
- Where: temporary step markers in Filesystem::unmount showed the hang after the directory connections were closed and the volume dropped, inside VolumesDirectory::terminate, which runs FxVolume::terminate. Markers on FxVolume::terminate's steps (background task, scope.shutdown + NodeCache::terminate + scope.wait, Epoch barrier, flush_all_files, graveyard flush, sync) were added but no hang occurred in the 1909-round run that carried them, so the stalled step is not yet known.
- Suspects: FxVolume::terminate relies on scope.shutdown closing open connections and on NodeCache::terminate breaking strong references (pager FileHolder) so that scope.wait and the node drops complete. The in-process fidl stand-in serves connections differently from Fuchsia channels, and the pager's VMO_ZERO_CHILDREN handling skips work once the scope is shutting down. A connection or task that holds an active guard, or a node that never drops, would leave scope.wait or the later take_device pending with no wakeup.

How to investigate: run the stress command above in Lima; when nextest reports SLOW for a test, attach with sudo gdb -p <pid> -batch -ex "thread apply all bt" (gdb is installed in the default Lima VM), and read #[unsafe(no_mangle)] AtomicU32 step markers with gdb print, as was done for UNMOUNT_STEP. Commit 3198500 has the markers removed; re-add them to FxVolume::terminate to find the step.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The root cause is identified and recorded
- [x] #2 cargo nextest run -p fxdax -j 24 --stress-duration 2h passes with no hang on Linux (Lima)
- [x] #3 The same stress run passes on macOS
- [x] #4 A test reproduces the cause deterministically, or the notes explain why one cannot
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fix in fxdax's own copy of fuchsia-async (crates/fuchsia-async, TASK-19 moved it out of vendor/): in portable/scope.rs's task OnDrop, wake the wakers release_cancel_guard returns instead of asserting there are none.
2. Regression test in runtime/scope.rs's tests: a task holding the scope's last guard, dropped unpolled while another task remains, must wake an on_no_tasks waiter (count waker). Watch it fail first.
3. Stress: cargo nextest run -p fxdax -j 24 --stress-duration 2h on Lima and macOS.
4. Workspace suites on both hosts.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Likely root cause, found while running TASK-18's tests in Lima (mknod_makes_special_files hung; memory storage, so not the new device): its stderr has a tokio worker panic in vendor/fuchsia/src/lib/fuchsia-async/src/runtime/portable/scope.rs:225, "assertion failed: ScopeState::release_cancel_guard(&mut state).is_empty()". In the task OnDrop, when the dropped task held an active guard, the code assumes the scope's wakers were already drained, but it drains them only if no tasks remain (all_tasks.is_empty()). If other tasks remain and releasing the guard brings guards to 0 while something waits on the scope (FxVolume::terminate's scope.shutdown() + scope.wait()), release_cancel_guard returns that waiter's waker, the assert panics on the executor thread, and the waker is dropped unwoken: scope.wait() never completes and unmount hangs with every thread idle, as observed. The portable runtime is only used off Fuchsia, which would explain upstream not seeing it. Fix: wake the wakers release_cancel_guard returns instead of asserting there are none (a patch to vendored fuchsia-async, through the kernel-patches mechanism or upstream). Earlier hangs were diagnosed with gdb only; their test stderr, which would show this panic, was not looked at.

Regression test runtime::scope::tests::task_dropped_with_last_guard_wakes_waiters (crates/fuchsia-async/src/runtime/scope.rs) failed before the fix with the same assertion (portable/scope.rs:225) and an unwoken waiter (wakes 0, expected 1), and passes after it. The fix is in crates/fuchsia-async/src/runtime/portable/scope.rs, whose leading comment records the deviation. Upstream main still has the assert (checked against fuchsia.googlesource.com), so it is worth sending upstream.

Stress with the fix: cargo nextest run -p fxdax -j 24 --stress-duration 2h ran 2108 rounds on Linux (Lima) and 2248 on macOS with no hang, failure or SLOW test, against hangs at rounds 20, 196 and 590 before (one 1909-round run without). The runs were stopped before the 2h mark (the host had slept, pausing their clocks) at the user's request, who accepted these counts. Workspace suites pass on macOS and Linux. DoD#2 (kernel-cfg.py --check) is obsolete: TASK-19 removed the script; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Root cause: fuchsia-async's portable scope asserted, when a task holding the scope's last active guard was dropped while other tasks remained, that releasing the guard woke no one; but FxVolume::terminate's scope.wait() was waiting, so the assert panicked on an executor thread and dropped the waiter's waker, and unmount hung with every thread idle. Fixed in fxdax's copy of fuchsia-async (crates/fuchsia-async/src/runtime/portable/scope.rs) by waking the wakers the release returns; the deviation is noted in the file, and upstream main still has the assert. Verified by a deterministic regression test (runtime::scope::tests::task_dropped_with_last_guard_wakes_waiters), which failed with the same assert before the fix, and by over 2100 stress rounds per host with no hang. Not committed.
<!-- SECTION:FINAL_SUMMARY:END -->
