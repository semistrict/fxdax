---
id: TASK-5
title: 'virtio-fs server: core FUSE ops on Fxfs'
status: Done
assignee:
  - '@claude'
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 23:53'
labels: []
milestone: m-0
dependencies:
  - TASK-1
  - TASK-2
type: feature
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The server that libkrun calls for guest filesystem requests, backed by FxVolume/FxDirectory/FxFile. Reads and writes go through kernel streams, which block on the pager, so they must not run on the Fxfs executor threads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A synchronous, errno-returning filesystem API over FxVolume/FxDirectory/FxFile provides lookup, forget, getattr, setattr (size, mode, times), open (truncate), create, read, write, fsync, fallocate, statfs, opendir, readdir, releasedir, mkdir, mknod, unlink, rmdir, rename, symlink, readlink and link, each shown by a test of that API; FUSE init, flush and release need no filesystem work and are answered by the adapter, as other virtio-fs backends do
- [x] #2 A libkrun FsBackend adapter on vendor/libkrun branch fxdax maps FUSE requests onto that API, with FUSE node ids mapped to Fxfs object ids and lookup counts
- [x] #3 Blocking stream I/O never runs on an Fxfs executor thread, asserted in debug builds
- [x] #4 Data written through the API survives unmount and remount and passes fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Split the work (doc-1): a FUSE-shaped synchronous API in fxdax, and a thin libkrun FsBackend adapter.

1. Filesystem info: port FilesystemInfo, FxVolume::filesystem_info_for_volume and the nodes' query_filesystem (now compiled out), and Directory.QueryFilesystem in the in-process fuchsia.io, for statfs.
2. File.LinkInto in the in-process fuchsia.io (upstream's handle_link_into), for FUSE link by inode.
3. New crate crates/fxdax: `Filesystem` owns a fuchsia-async SendExecutor on its own threads and opens/formats an Fxfs volume on a block device (FileBackedDevice for images, FakeDevice in tests), as the test fixture does. Operations run their futures on that executor and block the calling (virtio-fs worker) thread on the result.
4. crates/fxdax `vfs`: a FUSE-shaped `Vfs` (Attr, SetAttr, Creds, DirEntry, StatFs, Linux errno errors), acting as a fuchsia.io client of Fxfs like Starnix's remote fs: a node table maps FUSE inode numbers (Fxfs object ids; the root is 1) to open node connections with lookup counts; directory reads snapshot entries at opendir; file I/O uses the file's zx::Stream directly on the calling thread, with a debug assertion that it is not an executor thread.
5. Tests of every operation through the API, and persistence across close/reopen with fsck.
6. libkrun (vendor/libkrun, branch fxdax): FsBackend::Fxdax and devices/src/virtio/fs/fxdax.rs, behind an `fxdax` feature; DAX mapping stays ENOSYS until TASK-6. Build it with the feature on macOS and Linux.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Tests target the fxdax crate API directly; FUSE-level coverage comes from the libkrun e2e test (TASK-10) (doc-1).

- AC1 reworded: init/flush/release are adapter no-ops (as in other virtio-fs backends), so the API has no methods for them.
- crates/fxdax: `Vfs` is a fuchsia.io client of the volume, like Starnix remote fs/zxio: nodes are opened by name with FLAG_SEND_REPRESENTATION (node proxy, then cast to the protocol via its channel); file I/O through the file's zx::Stream on the caller thread; `Executor` runs Fxfs on a SendExecutor of its own. Errors map zx::Status to Linux errnos with fdio's table (sdk/lib/fdio/fdio_unistd.cc).
- Stand-in additions: fidl Proxy trait/AsyncChannel, fio Node proxy, FilesystemInfo + Directory.QueryFilesystem, File.LinkInto; fidl-fuchsia-fs VfsType; FxVolume filesystem info and nodes' query_filesystem ported.
- The stream-I/O assertion is a plain assert (always on), covered by stream_io_on_an_executor_thread_panics.
- libkrun branch fxdax: an fxdax backend (feature fxdax, krun_add_virtiofs_fxdax, KRUN_FEATURE_FXDAX); DAX mapping returns ENOSYS until TASK-6. Its guest e2e test comes in TASK-10.
- Validation: scripts/test.sh macOS 1730 passed, Linux 1729 passed; libkrun `cargo build -p libkrun --features fxdax,blk,net` and without fxdax on macOS, with fxdax on Linux.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added crates/fxdax, a FUSE-shaped synchronous Vfs over an Fxfs volume (a fuchsia.io client, with stream I/O on the calling thread and Fxfs on its own executor), tested op by op and across remount + fsck; and libkrun's fxdax FsBackend adapter (branch fxdax), built on macOS and Linux. Verified: 1730/1729 workspace tests pass on macOS/Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
