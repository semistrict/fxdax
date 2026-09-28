---
id: doc-1
title: libkrun virtio-fs server interface
type: specification
created_date: '2026-09-28 02:44'
updated_date: '2026-09-28 23:53'
---
Survey for TASK-1. Paths are in `vendor/libkrun` (semistrict/libkrun) unless noted.

## Device, worker and threading

- `src/devices/src/virtio/fs/device.rs`: `Fs` is the virtio device. Its backend is `FsBackend` (`Null`, `Passthrough(Config)`, and further backends behind features). `activate` starts one `FsWorker`.
- `src/devices/src/virtio/fs/worker.rs`: `FsWorker::new` builds `FsServer`, an enum of `Server<AugmentFs<T>>` per backend. `FsWorker::run` spawns **one thread per device** ("fs worker"). It epolls the high-priority and request queue eventfds and handles descriptors **sequentially**: `process_queue` → `Server::handle_message` → `FileSystem` method → reply → `add_used` → interrupt.
- So every FUSE request runs synchronously on that worker thread, one at a time. That thread is not an Fxfs executor thread. The adapter can block on Fxfs futures there (as `FileSynchronousProxy` does) and can do blocking stream I/O without starving the pager.
- DAX faults do not reach the worker. They happen on vCPU threads (HVF data-abort exits) or in the kernel on behalf of KVM (see below).

## Server and FileSystem trait

- `src/devices/src/virtio/fs/server.rs`: `Server<F: FileSystem>` decodes FUSE (`handle_message`, opcode match at ~l.100-200) and encodes replies.
  - `init` (l.854) negotiates FUSE 7.x. The server always offers ASYNC_READ, PARALLEL_DIROPS, BIG_WRITES, AUTO_INVAL_DATA, ASYNC_DIO, HAS_IOCTL_DIR, ATOMIC_O_TRUNC, MAX_PAGES, SUBMOUNTS, HANDLE_KILLPRIV_V2, INIT_EXT and ALLOW_IDMAP (plus SECURITY_CTX on macOS), and adds whatever `FileSystem::init` wants.
  - It replies `max_write = MAX_BUFFER_SIZE`, `max_pages` from the host page size and `map_alignment = 0`.
  - `SetupMapping`/`RemoveMapping` are dispatched only when the device has a shm region. The base passed is `shm.host_addr` on Linux and `shm.guest_addr` on macOS.
- `src/devices/src/virtio/fs/filesystem.rs`: `trait FileSystem` (l.351), all methods `&self`.
  - `type Inode`/`type Handle`: `From<u64> + Into<u64>`. Lookup-count semantics are per FUSE: every returned `Entry` takes a reference, and `forget` drops them.
  - `read`/`write` take `ZeroCopyWriter`/`ZeroCopyReader`, which also implement `io::Write`/`io::Read`. A backend may copy through a `Vec`.
  - `setupmapping(ctx, inode, handle, foffset, len, flags, moffset, shm_base, shm_size, [macOS] map_sender)` and `removemapping(ctx, Vec<RemovemappingOne>, shm_base, shm_size, [macOS] map_sender)` (l.1134-1160).
- `src/devices/src/virtio/fs/augment_fs.rs`: `AugmentFs<T>` wraps the backend to add virtual entries. Their inode numbers come from an `InodeAllocator` that the backend must leave free (for example `InodeAllocator::starting_at(1 << 32)`).

## FUSE operations a usable mount needs

A DAX backend that passes a guest e2e test (mount with `dax=always`, DAX checks, mmap and write() I/O, namespace operations, truncating a mapped file, write-back on REMOVEMAPPING). It implements exactly:

- **Session:** init, destroy.
- **Namespace:** lookup, forget, getattr, setattr, readlink, symlink, mknod, mkdir, unlink, rmdir, rename, link.
- **Files:** open, create, read, write, flush, fsync, release, statfs.
- **Directories:** opendir, readdir, readdirplus, fsyncdir, releasedir.
- **DAX:** setupmapping, removemapping.

Everything else takes the trait's default, ENOSYS: xattrs, access, lseek, fallocate, copy_file_range, ioctl, locks. The Linux client copes with that. fxdax should implement the same set, plus fallocate (Fxfs supports it and TASK-5 asks for it).

## Linux guest DAX behaviour (`~/src/linux/fs/fuse/dax.c`)

- The window is split into 2 MiB ranges (`FUSE_DAX_SHIFT = 21`). The guest always sends SETUPMAPPING with `len = 2 MiB` and a 2 MiB-aligned `foffset` and `moffset`, even when the range extends past EOF.
- Mappings start read-only (`FUSE_SETUPMAPPING_FLAG_READ`). The first write sends SETUPMAPPING again for the same range with `FLAG_WRITE` (`fuse_upgrade_dax_mapping`), so a setupmapping must be able to replace an existing mapping in place.
- REMOVEMAPPING is batched (up to `FUSE_REMOVEMAPPING_MAX_ENTRY` per request). A reclaim worker frees ranges when free ranges fall under 20%, and inode eviction and truncation also remove mappings.
- `fuse_iomap_begin`: with no mapping and `pos >= i_size`, reads return a hole without asking the server. So the guest itself bounds accesses by `i_size`. Pages past EOF inside a mapped 2 MiB range are only reached if the guest's size is stale.
- `write()` on a DAX file goes through the mapping unless it extends the file, in which case it becomes FUSE_WRITE (`fuse_dax_direct_write`). The server therefore sees both paths for the same file.
- `map_alignment` must be ≤ 21. libkrun sends 0 without `FUSE_MAP_ALIGNMENT`, so the guest assumes its own page size.

## DAX window plumbing

- **Window creation:** `src/vmm/src/device_manager/shm.rs` gives each fs device with `shm_size` a page-aligned guest-physical region after RAM. `src/vmm/src/builder.rs:1815` adds all shm regions to guest memory as ordinary anonymous regions, and l.2332 hands the device a `VirtioShmRegion { host_addr, guest_addr, size }`. `krun_add_virtiofs*` in `src/libkrun/src/lib.rs` takes `shm_size` from the caller, where 0 means no DAX; `krun_set_root` uses 512 MiB.
- **Linux (KVM):** the region is a KVM memslot over `host_addr`. The passthrough backend `mmap(MAP_SHARED|MAP_FIXED)`s a file over `host_addr + moffset`, and KVM follows the host mapping. Unmapping replaces the range with `PROT_NONE` anonymous memory (`linux/passthrough.rs` ~l.2027).
- **macOS (HVF):** the device sends `WorkerMessage::GpuAddMapping(reply, host_addr, guest_addr, len)` / `GpuRemoveMapping` to the VMM worker thread (`src/vmm/src/worker.rs`). That thread calls `hv_vm_unmap` then `hv_vm_map` with RWX (`src/vmm/src/macos/vstate.rs:133`, `src/hvf/src/lib.rs:310`). The worker is started only when a GPU or an fs shm window exists (`lib.rs:3237`, d5ddac7).
- **HVF faults:** `EC_DATAABORT` handling (`src/hvf/src/lib.rs:927`) checks write faults against libkrun's snapshot dirty tracker (`dirty_tracking_handle_write_fault`, which uses `hv_vm_protect`). All other aborts become MMIO exits. There is no hook yet for "unmapped IPA in a DAX window → ask the pager". That's TASK-8.
- **IPA granule:** libkrun does not call `hv_vm_config_set_ipa_granule`, so HVF maps in 16 KiB units.

## How fxdax fits

- **Backend:** add an `FsBackend::Fxdax(..)` variant and a `devices/src/virtio/fs/fxdax.rs` adapter behind an `fxdax` feature. The adapter stays thin; the filesystem semantics live in an fxdax crate with a synchronous, errno-returning API over FxVolume/FxDirectory/FxFile. TASK-5's tests can then drive that crate directly, without libkrun.
- **Inode numbers:** Fxfs object ids are not FUSE node ids. The root directory is not object 1, and encrypted volumes use 64-bit ids from `LastObjectId::Encrypted`, so the adapter needs a nodeid ↔ object id map with lookup counts. That also leaves room for `AugmentFs` virtual inodes.
- **DAX mappings:** the kernel's `GuestPhysicalAspace` (`~/src/fxdax/crates/zircon/src/hypervisor/aspace.rs`) is a host-address window mapped lazily on `page_fault(gpaddr, write)`, with dirty-correct write faults. SETUPMAPPING maps a range of the file's pager-backed VMO into that aspace without populating it. Guest accesses then fault into the pager:
  - on HVF, via data-abort exits (TASK-8);
  - on KVM, via the host fault on the memslot's host mapping (TASK-9).
  The DAX window's host memory must be the aspace window rather than libkrun's anonymous shm mapping.
- **Mappings past EOF:** Fxfs file VMOs are `UNBOUNDED` (`pager.rs:1307`), so a 2 MiB mapping past EOF stays inside the VMO. Pages past the stream size are zero-filled, and the guest bounds access by `i_size`. That is the input to TASK-7.

## Open questions

- Whether fxdax's libkrun changes go on a branch of semistrict/libkrun, vendored as a submodule of fxdax.
