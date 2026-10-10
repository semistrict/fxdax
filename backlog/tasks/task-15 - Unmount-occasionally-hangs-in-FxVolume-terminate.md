---
id: TASK-15
title: 'Unmount occasionally hangs in FxVolume::terminate'
status: To Do
assignee: []
created_date: '2026-10-10 00:31'
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
- [ ] #1 The root cause is identified and recorded
- [ ] #2 cargo nextest run -p fxdax -j 24 --stress-duration 2h passes with no hang on Linux (Lima)
- [ ] #3 The same stress run passes on macOS
- [ ] #4 A test reproduces the cause deterministically, or the notes explain why one cannot
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
