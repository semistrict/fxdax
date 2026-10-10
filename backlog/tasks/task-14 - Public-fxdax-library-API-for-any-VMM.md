---
id: TASK-14
title: Public fxdax library API for any VMM
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-09 21:58'
updated_date: '2026-10-10 00:30'
labels: []
dependencies: []
type: feature
ordinal: 14000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax is consumed only by libkrun, and its API shows it: it leaks storage_device::DeviceHolder, anyhow and the Zircon SecondStage (libc prot ints), uses bare u64 for inodes and handles, has no vectored I/O for zero-copy guest buffers, hard-codes executor threads and kernel memory, and makes each VMM hand-wire DAX faults per host (UffdSecondStage + FaultThread on Linux, a Zircon trait on macOS). Any Rust VMM (libkrun, crosvm, firecracker, cloud-hypervisor, Dragonball) should be able to embed it from documented, typed, self-contained APIs. Scope decided with the user: Rust API only (no fuse-backend-rs or C ABI); libkrun adapter and guest test ported and committed locally on its fxdax branch, not pushed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The public API exposes no types from storage_device, anyhow, zircon or fidl; storage, errors, kernel configuration and the DAX second stage are fxdax types
- [x] #2 Inodes and directory handles are distinct newtypes; rename flags and file types are typed
- [x] #3 Reads and writes accept vectored buffers
- [x] #4 A VMM sets up DAX with one call per hypervisor model: a second stage it implements (HVF-like), or on Linux host memory the hypervisor follows (KVM), with fault serving owned by the library
- [x] #5 The filesystem tests run as integration tests against the public API only
- [x] #6 Crate docs explain how a VMM integrates: FUSE op mapping, DAX models, threading, errors; missing_docs is denied
- [x] #7 libkrun builds against the new API and its virtiofs-fxdax and virtiofs-fxdax-nodax guest tests pass on macOS
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Modules: filesystem.rs (Filesystem, Options), node.rs (Inode, DirHandle, Attr, FileType, SetAttr, SetTime, Owner, DirEntry, DirEntries, StatFs, RenameFlags), storage.rs (Storage: file, open, create, memory; check), error.rs (Errno with named consts; Error for setup), dax.rs (DaxWindow, SecondStage, Protection, Access; uffd private), kernel.rs (Config, configure, page_size), executor.rs. Flat re-exports at the root.
2. Timestamps are SystemTime. mknod takes FileType + perm. allocate replaces fallocate(mode). open() removed (truncate is setattr size 0). readdir returns DirEntries (Arc slice + start, no copy).
3. DAX: Filesystem::create_dax_window(size, Option<Arc<dyn SecondStage>>) and, on Linux, unsafe create_dax_window_in_host_memory(base, size), which owns the userfaultfd fault thread for the life of the mount. fxdax SecondStage adapts to the Zircon trait internally.
4. Move vfs/tests.rs to tests/filesystem.rs using only the public API; port memory_pressure.rs; add vectored I/O and DAX tests.
5. Crate docs with VMM integration guide; deny(missing_docs).
6. Port libkrun adapter (devices/src/virtio/fs/fxdax.rs) and test_virtiofs_fxdax.rs; commit on submodule branch fxdax; bump submodule. README crates/fxdax section.
7. Verify: scripts/test.sh on macOS, scripts/test-linux.sh (Lima), scripts/test-e2e.sh on macOS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax restructured into connection, dax (+uffd), error, executor, filesystem, kernel, node, storage; flat re-exports, deny(missing_docs). Errno/IntoErrno replace the public From<zx::Status>/From<fidl::Error> impls. Inode/DirHandle convert to and from u64 (FUSE servers such as libkrun require From<u64> + Into<u64> handle types). After unmount operations fail with ENOTCONN; the uffd fault thread lives until the Filesystem drops so a late guest fault fails loudly rather than hanging. Tests moved to tests/filesystem.rs on the public API; added vectored_io, storage_errors, dax_unmount_unmaps_the_window, errno_names, file_types, readdir past the end, tests/kernel.rs. open() and fallocate(mode) removed: the libkrun adapter does O_TRUNC as setattr size 0 and returns ENOTSUP for nonzero fallocate modes. libkrun adapter + virtiofs-fxdax test ported; macOS e2e: virtiofs-fxdax and virtiofs-fxdax-nodax pass.

Validation: fxdax 42 tests pass (macOS); clippy and rustdoc -D warnings clean for fxdax; scripts/test.sh macOS 1764 passed; scripts/test-e2e.sh macOS virtiofs-fxdax and -nodax pass; Lima scripts/test.sh 1765/1766 passed, contents_survive_remount hung. The hang is pre-existing: under cargo nextest run -p fxdax -j 24 --stress-duration, HEAD (df29131) hangs the same test at round 196, and this change hung it at rounds 590 and (in readdir) 20 and not at all in 1909 rounds. Diagnosis so far: the test thread waits in unmount, inside VolumesDirectory::terminate (FxVolume::terminate), with every executor thread idle. Linux libkrun e2e (scripts/test-e2e-linux.sh) not yet run, so the Linux adapter path is not yet compiled or verified; DoD #1 open until then.
<!-- SECTION:NOTES:END -->
