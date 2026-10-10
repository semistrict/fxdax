---
id: TASK-18
title: 'Production image device: direct, async I/O, hole-punching trim, clones'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 02:09'
updated_date: '2026-10-10 14:44'
labels: []
dependencies:
  - TASK-17
type: feature
ordinal: 18000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax stores images through Fuchsia's FileBackedDevice, which upstream uses only for host tools and tests: its reads and writes are blocking pread/pwrite on Fxfs's executor threads, so disk I/O still holds threads after TASK-16; its trim overwrites freed ranges with 0xab, which un-shares a cloned image's blocks and un-sparsens the file; and the host page cache caches every block a second time, beside the in-process kernel's page cache. The user asked for a production device that replaces and removes it, and for cheap clones of an image via the host filesystem. Investigation also found that the kernel port's page scanner makes mapped pages inaccessible to track accesses, so host I/O into Fxfs's transfer buffers can fail with EFAULT (a latent bug of the old device too). Do not commit (user instruction, as for TASK-16/17).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An image device opens images for direct I/O (O_DIRECT on Linux, F_NOCACHE on macOS), falling back to buffered I/O where the host filesystem refuses it
- [x] #2 Its reads, writes and flushes run off Fxfs's executor threads: io_uring on Linux, an I/O thread pool elsewhere; a dropped I/O future waits for the host to be done with its buffer
- [x] #3 Trim punches holes, and flush is fdatasync (F_FULLFSYNC on macOS), shown by tests
- [x] #4 Host I/O into transfer buffers whose pages the scanner made inaccessible succeeds, by faulting them in and retrying
- [x] #5 Storage::clone_to clones closed storage, and Filesystem::clone_storage_to a mounted filesystem after writing back its dirty data; both share blocks via clonefile/FICLONE where the host filesystem can, copy otherwise, and say which
- [x] #6 FileBackedDevice is compiled out of fxdax's build (kernel-cfg.py disable rule), with vendor/fuchsia unchanged
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
- [x] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #5 libkrun guest tests pass on macOS and Linux
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Dependency: io-uring (MIT/Apache-2.0) on Linux.
2. storage/io.rs: an I/O engine with async read_at, write_at, sync_data, punch_hole on a file: io_uring with a completion thread (Linux), a thread pool (elsewhere); ops whose future is dropped are waited for before their buffers are released.
3. storage/image.rs: ImageDevice: Device over the engine; direct I/O with fallback; EFAULT retry after touching the buffer's pages; trim punches holes (no-op where unsupported); flush fdatasync / F_FULLFSYNC; close flushes.
4. storage/mod.rs: Storage::file/open/create use ImageDevice and keep the file for clones; Storage::clone_to; Filesystem::clone_storage_to (flush all files, sync the filesystem with a device flush, then clone). clone: fclonefileat (macOS), FICLONE then copy_file_range (Linux); CloneKind reports which.
5. kernel-cfg.py: disable file_backed_device in storage_device; regenerate; --check.
6. Tests: device I/O, direct I/O on the target dir, hole punching (allocated blocks drop), concurrency on one thread, clones of closed and mounted filesystems (independent, contents equal; Reflink on APFS).
7. Verify macOS and Lima suites and guest tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: cargo nextest run --workspace passes on macOS (1804 passed, 4 skipped) and Linux in Lima (1807 passed, 4 skipped); clippy clean for fxdax; RUSTDOCFLAGS=-D warnings cargo doc -p fxdax passes; kernel-cfg.py --check passes; libkrun guest tests virtiofs-fxdax and virtiofs-fxdax-nodax pass on macOS (HVF) and Linux (KVM in Lima). AC#1's fallback to buffered I/O, where the host filesystem refuses direct I/O, has no test: neither test host's filesystem refuses it.

Superseded by TASK-19: fxdax now owns copies of the Fuchsia sources it builds, and FileBackedDevice is simply not among them (crates/storage_device/src/lib.rs notes it), so the kernel-cfg.py disable rule of AC#6 is gone with the script.

Pre-commit review fixes: io_uring submissions no longer panic under a lock (a split submission lock and in-flight table; EBUSY/EAGAIN/EINTR retried; an op is completed with the error only if it was never queued); the macOS pool unlocks its receiver while an op runs; F_FULLFSYNC falls back to fsync only when unsupported; the EFAULT retry budget resets after progress; a copied clone pauses the image's writes (storage/gate.rs, WriteGate) so it is the image at one time; copy_file_range falls back to reads and writes across filesystems; the clone's directory is fsynced; unmount publishes its completion while taking the mount (no early return for a concurrent unmount), closes despite DAX unmap errors, and the executor finishes running jobs before stopping, so dropping a filesystem mid-unmount no longer cancels the close.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Replaced FileBackedDevice with fxdax's ImageDevice (storage/image.rs): direct I/O (O_DIRECT / F_NOCACHE), I/O off Fxfs's threads (storage/io.rs: io_uring on Linux, a thread pool elsewhere; dropped futures wait for their buffers), hole-punching trim, fdatasync / F_FULLFSYNC flush, and EFAULT retries after touching buffer pages the scanner made inaccessible. Added Storage::clone_to and Filesystem::clone_storage_to (storage/clone.rs: clonefile / FICLONE, else a sparse copy; CloneKind says which). FileBackedDevice is compiled out by a kernel-cfg.py disable rule. Verified by the workspace suites on macOS and Linux, new image and storage tests, and libkrun's guest tests on both hosts. Not committed, per the user.
<!-- SECTION:FINAL_SUMMARY:END -->
