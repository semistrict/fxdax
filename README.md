# fxdax

Fuchsia's Fxfs filesystem, running outside Fuchsia so that virtual machines
can use it over virtio-fs with DAX, the way Fuchsia programs use it through
Zircon pagers.

A guest's file pages are Fxfs's own pages. Fxfs's pager supplies them, tracks
which ones the guest dirties, and writes those back, as it does on Fuchsia.
To make that possible, fxdax runs Fxfs's code largely as upstream wrote it,
on a port of the parts of the Zircon kernel that give Fuchsia its memory
semantics, inside the VMM's own process.

## How it fits together

```
guest (Linux)            virtio-fs: FUSE requests, and DAX loads and stores
  │
libkrun                  vendor/libkrun: the virtio-fs device and its fxdax backend
  │
fxdax::Filesystem        crates/fxdax: a FUSE-shaped client of the volume
  │  fuchsia.io, in-process
Fxfs's platform layer    crates/fxfs-platform: files, directories, the pager
  │
Fxfs                     crates/fxfs: journal, object store, allocator
  │
image file               crates/fxdax: ImageDevice (direct, async I/O)

in-process Zircon        crates/zircon: pager-backed VMOs, dirty tracking,
                         page queues and eviction, guest physical address
                         spaces
```

- **FUSE requests:** a request such as `LOOKUP` or `WRITE` becomes a
  `Filesystem` call. `Filesystem` talks fuchsia.io to the volume, as Starnix's
  remote filesystems and fdio do, and moves file contents through each file's
  Zircon stream.
- **DAX:** a DAX mapping (`FUSE_SETUPMAPPING`) maps a range of the file's
  pager-backed VMO into a DAX window, a guest physical address space of the
  in-process kernel. The guest's first access to a page faults. The kernel
  resolves the fault as it would for any mapping of the VMO: Fxfs's pager
  reads the page in, or marks it dirty before the first store.
- **Hypervisor mirror:** the kernel maps pages at the window's host address,
  and makes the same changes to the hypervisor's translation of the window (a
  "second stage"). On macOS this is HVF's stage 2. On Linux it is host memory
  that KVM follows, which fxdax keeps in step with userfaultfd.

## Where the code comes from

Code falls into four kinds:

- Fuchsia sources, copied;
- ports of the Zircon kernel;
- stand-ins for what exists only inside Fuchsia's build;
- fxdax's own code.

All of it lives under `crates/`. Code copied, ported or excerpted from Fuchsia
follows the revision in the root `Cargo.toml`'s
`workspace.metadata.fuchsia.commit`.

### Fuchsia, copied

Most crates are Fuchsia's own, with only the files the workspace builds, and
their tests:

- Fxfs;
- its libraries (`storage_device`, `fscrypt`, `fsverity-merkle` and others);
- the Rust runtime crates (`fuchsia-async`, `fuchsia-sync` and others);
- the SDK's `zx` crates;
- parts of the vfs library.

Each crate's `Cargo.toml` cites the Fuchsia directory it is copied from. Most
files are as upstream wrote them. Those that aren't say how they deviate in
their leading comment:

- The runtime crates are built for the host only, without Fuchsia's runtime
  and handles.
- Some code only builds what fxdax needs, such as VMO-backed buffers and
  pager-backed files, when `target_os = "fuchsia"`. In `storage_device`,
  `storage_units`, Fxfs's `errors.rs` and the vfs library's token registry,
  the Fuchsia code paths the in-process kernel can build are built
  unconditionally. What it can't build, such as FIDL protocols over channels
  or tests that need Fuchsia's component framework, is removed from those
  files and from the platform layer's directories, paged object handles and
  pager.
- `storage_device` has no `FileBackedDevice`: fxdax has its own image device
  (see `crates/fxdax`).
- Paged object handles support pages larger than blocks: Apple silicon hosts
  have 16 KiB pages, but Fxfs keeps its 4 KiB blocks.
- Directories and the token registry's test mocks have no watchers, which are
  channels the in-process vfs doesn't have.
- Fxfs can snapshot a volume (`RootVolume::snapshot_volume`), which upstream
  can't. The snapshot shares the source's layer files, by reference count,
  and its data extents. The allocator records a store's allocations under a
  data owner that a snapshot changes; the source's owner at the snapshot is
  frozen, its extents never deallocated one by one while any store shares
  them, and freed once none does. Blocks written in place (preallocated
  files) are written to new blocks where a snapshot shares them. fsck checks
  shared extents, and the on-disk format is version 59.
- Files share the pages of the data frozen owners hold: the platform layer
  caches those pages by device block, one cache VMO per frozen owner
  (`shared_blocks.rs`), and supplies files with them as shared pages, so a
  volume and its snapshots use one page of memory per block they read.

### `crates/zircon`: the Zircon kernel, ported

Fxfs's pager depends on Zircon's pager semantics:

- VMOs whose pages a userspace pager supplies;
- dirty tracking that asks the pager before a clean page is written;
- writeback that write-protects pages again;
- page queues and eviction under memory pressure.

Rather than rewrite Fxfs for a host's page cache, fxdax ports that part of
the kernel. It runs inside the host process:

- `vm/` holds the VMM: VMOs, their page lists and copy-on-write pages, VMARs
  and mappings, the PMM, page queues, the scanner and the evictor.
- `object/` holds the dispatchers for VMOs, VMARs, pagers, ports, streams and
  events.
- `syscalls/` holds the syscalls over them, exported with their C names.
- `hypervisor/` holds guest physical address spaces.

It is ported from `zircon/kernel`, keeping Zircon's structure and names.
Zircon's C++ is translated, and its Rust files are copied. Zircon's own unit
tests come along where they can run. `crates/zircon-core-tests` ports
Zircon's core tests (`zircon/system/utest/core`), which drive the kernel
through the `zx` crate as a Fuchsia program would.

What changes on a host:

- **Architecture layer (`arch/host`):**
  - Physical memory is a shared memory object (a memfd or POSIX shared
    memory), and its "physmap" is a mapping of all of it.
  - An address space is a reserved range of host virtual memory. Mapping a
    page maps its part of physical memory there with `mmap(MAP_FIXED)`, and
    unmapping restores the reservation.
  - Faults arrive as `SIGSEGV`/`SIGBUS` and go to the VMM's fault handler, as
    a data abort would.
  - Accessed flags are emulated by making pages inaccessible.
- **Syscalls:** `zx-sys` declares the syscalls for the host. Fuchsia's `zx`
  crate calls them unchanged, and they resolve to this kernel instead of
  Fuchsia's vDSO.
- **Page size:** the page size is the host's (the `page` crate), 16 KiB on
  Apple silicon, rather than Zircon's fixed 4 KiB.
- **Guest windows:** a guest physical address space is a window of host
  memory. Its mappings can also be made to a hypervisor's `SecondStage`,
  since a hypervisor may not follow the host's mappings as they change.
- **Futures in place of threads:** where a Zircon thread blocks, on a page
  request, a page allocation or a stream's ordering, a future can wait
  instead, with its waker registered where the thread would block
  (`kernel/wakers.rs`). Stream reads and writes of host memory, VMO zeroing
  and guest faults have async versions (`stream_readv_at_async`,
  `stream_writev_at_async`, `page_fault_async`) that run the same steps as
  the synchronous ones between waits. One thread serves their deadlines.
- **Not ported:** VMO children, the kernel address space, and the
  interrupt-controller parts of the hypervisor. Fxfs's code paths that need
  them are left out.
- **Shared pages, beyond Zircon:** a pager can supply a VMO with the clean
  pages of another pager-backed VMO, which the two then share
  (`zx_pager_supply_shared_pages`). The page stays its lender's, in its page
  queue and counted once; the borrower maps it read-only, copies it before
  writing or pinning it (raising the usual DIRTY request), and gives it back
  rather than freeing it. The lender takes it back from its borrowers before
  it writes, pins or evicts it (`VmCowPages::lend_pages`).

### `crates/`: stand-ins for Fuchsia-only pieces

Some crates only exist inside Fuchsia's build, or only work on Fuchsia. Each
of these stands in with the items the code copied from Fuchsia uses:

| Crate | Stands in for | How |
|---|---|---|
| `fidl` | FIDL runtime | No channels or wire format: a proxy calls the object serving its protocol in-process. Handles are the in-process kernel's. |
| `fidl-fuchsia-io`, `-fs`, `-fs-startup`, `-fxfs`, `-memorypressure`, `-diagnostics-types` | generated FIDL bindings | Written to the shape of the generated types, for the types and protocols Fxfs uses. fuchsia.io's `File`, `Directory` and `Symlink` protocols, and fuchsia.memorypressure's `Provider` and `Watcher`, are served in-process. |
| `memory-pressure-signaler` | the memory pressure signaler component (`src/developer/memory/pressure_signaler`) | Ported: it watches the kernel's memory pressure events (`zx_system_get_event`, `zx_object_wait_many`) and serves `fuchsia.memorypressure/Provider` to the process, as the component does to the system. No crash reports. |
| `vfs` | `src/storage/lib/vfs/rust` | Some files copied: execution scopes, paths, temporary clones, the token registry, and parts of `directory`. Node, file, directory and symlink connections are excerpts that serve in-process proxies. |
| `flex_client` | FDomain's `flex.rs` | Re-exports FIDL, which is all the copied code needs. |
| `fuchsia-async-kernel` | — | fuchsia-async plus its Fuchsia executor's port packet receivers, which Fxfs's pager uses. On hosts, fuchsia-async's portable executor has no ports. |
| `fuchsia-runtime` | process runtime | The runtime the in-process kernel provides. |
| `fuchsia-inspect`, `diagnostics-log`, `refaults_vmo` | diagnostics | No-ops, stderr logging, and an in-memory counter. |
| `fuchsia-fs` | `src/lib/fuchsia-fs` | What fxdax's tests use. |
| `bssl-crypto`, `mundane` | BoringSSL bindings | SHA-256/512 on the `sha2` crate. |
| `fxfs-unicode-gen` | build-time generated tables | Tables generated once from UCD 12.1.0 by Fxfs's own generator (`scripts/generate-unicode-tables.sh`). |

### `crates/fxfs-platform`: Fxfs's platform layer, in part

Fxfs's platform layer (`src/storage/fxfs/platform`) holds its files,
directories and pager.

**Copied:** the directory entry cache, the node cache, symlinks, and, with the
deviations above, directories, paged object handles and the pager.

**Excerpts, copied verbatim except as noted:**

- `file.rs`: FxFile.
  - `get_backing_memory` hands out the file's own VMO rather than a reference
    child, as the kernel has no VMO children. Whoever maps it keeps the file
    open.
  - `mark_dirty` refuses whole pages past the last page that fits in the
    maximum file size.
  - fsverity is left out.
- `volume.rs`: FxVolume, with its background task, which flushes its files
  every 20 s (5 s under warning memory pressure, 1.5 s under critical) and
  flushes dirty pages as soon as pressure turns critical.
  - The task's timer is fused and replaced to reset it, as the host's
    fuchsia-async timer can't be reset.
  - Blobs, profiles, inspect and the fuchsia.fxfs protocols are not ported.
- `volumes_directory.rs`: VolumesDirectory, which owns the mounted volumes and
  their memory pressure monitor, counts the pages they dirty, and under
  critical pressure flushes them all before letting more pages be dirtied
  once the count reaches 1% of memory.
  - Volumes aren't served over FIDL, so neither are the methods serving them,
    nor auto-unmounting, checking, removing or installing volumes.
- `memory_pressure.rs`: the MemoryPressureMonitor, verbatim except that its
  watcher is served in-process, and registers with the process's memory
  pressure signaler.
- `errors.rs`: without blob errors.
- `profile.rs`: only the pager's `Recorder` trait.
- `testing.rs`: the test fixture, serving the volume in-process.
- `directory_tests.rs`: fxdax's own tests of cross-directory renames and hard
  links, which upstream covers only in its conformance suites, and those
  aren't copied.

### `crates/fxdax`: fxdax's own code

`fxdax` is the library a VMM embeds to serve Fxfs to a guest over virtio-fs.
Its crate docs (`cargo doc -p fxdax --open`) are the guide for VMM authors:
serving a device's requests, setting up DAX for each kind of hypervisor, async,
errors, images and their volumes, and storage. Its public API names no Fuchsia, Zircon or FIDL type, and
every operation that can wait is an `async fn` that holds no thread while it
waits.

- `fuse/` holds `fuse::Server`, which decodes a virtio-fs device's FUSE
  requests and replies to them. A device hands it each descriptor chain as
  volatile slices of guest memory: the request's and the reply's. It is
  ported from libkrun's FUSE server (from crosvm) and its former fxdax
  adapter.
- `memory.rs` holds `VolatileSlice` and `VolatileSliceMut`, memory fxdax only
  copies into and out of, as a guest may change it concurrently.
- `image.rs` holds `Image`: a mounted Fxfs image, whose volumes are
  filesystems of their own, sharing its journal, space and executor. It
  creates, lists, opens, snapshots and deletes volumes, mounting them through a
  VolumesDirectory, as Fxfs's component does, so their background tasks and
  dirty limit follow the kernel's memory pressure; closing it unmounts the
  volumes still mounted.
- `filesystem.rs` holds `Filesystem`, the FUSE-shaped view of a volume the
  server serves, and its `Options`. A filesystem mounted from storage is the
  `default` volume of an image of its own.
  - It works with inode numbers and lookup counts, as FUSE does.
  - Every looked-up inode keeps a fuchsia.io connection to its node
    (`connection.rs`).
  - Inode numbers are Fxfs object ids, except that the root is 1.
  - Reads and writes copy straight between volatile slices, such as a
    request's guest memory, and the files' pages.
- `node.rs` holds the types operations take and give: `Inode`, `DirHandle`,
  `Attr`, `FileType`, `SetAttr` and the others.
- `error.rs` holds `Errno`, Linux's errno values on every host, mapped from
  Zircon statuses with fdio's table, and `Error`, for setting up.
- `storage/` holds `Storage`: an image file, or memory. It checks
  filesystems (fsck), and clones images.
  - `image.rs` is the device on image files, in place of Fuchsia's
    `FileBackedDevice`, which is for host tools and tests. I/O bypasses the
    host's page cache where the host filesystem allows (`O_DIRECT`,
    `F_NOCACHE`), since the in-process kernel caches files' pages already.
    Trim punches holes, and flush is `fdatasync` (`F_FULLFSYNC` on macOS).
    Host I/O into transfer buffers whose pages the kernel has made
    inaccessible, or maps read-only as the zero page, is retried after
    faulting them in.
  - `io.rs` runs that I/O off Fxfs's executor threads: on an io_uring on
    Linux, and a small thread pool elsewhere.
  - `clone.rs` clones an image file, sharing its blocks where the host
    filesystem can (`clonefile` on APFS, `FICLONE` on btrfs and XFS), and
    copying its data, holes kept, elsewhere. `Image::clone_storage_to` clones
    a mounted image after writing back its volumes' dirty data, and a copy
    pauses the image's writes, so it is the image as it was at one time.
- `kernel.rs` configures the in-process kernel: its memory, which files' pages
  come from, and its memory pressure thresholds.
- `dax.rs` holds DAX windows and the `SecondStage` trait a VMM implements for a
  hypervisor like HVF. It documents how pages past the end of a file behave,
  and why DAX doesn't need `ZX_VMO_FAULT_BEYOND_STREAM_SIZE` (decision-2 in the
  backlog).
- `dax/uffd.rs` holds the second stage for a hypervisor that follows host
  memory, such as KVM, described below.
- `executor.rs` holds the fuchsia-async executor Fxfs runs on. Its threads
  are separate from the VMM's, whose futures await the operations it runs.

**DAX on Linux.** KVM maps a guest's memory from the host memory backing its
memory slot, and follows the host's mappings. On arm64, though, a fault it
can't resolve there is reported only as `EFAULT`, with no address. So that
memory must never fault that way. A VMM hands it to the filesystem
(`Filesystem::create_dax_window_in_host_memory`), which keeps it like this:

- Pages the window hasn't mapped are anonymous memory registered with
  userfaultfd for missing faults.
- Mapped pages are the kernel's pages mapped again (`mremap` with
  `old_size` 0). They stay read-write, and userfaultfd write-protects them
  while the kernel maps them read-only.
- A fault thread of the filesystem's passes each event to the window,
  resolving them concurrently, as futures, so a fault waiting for the pager
  holds up no other.

x86 KVM faults pages in for writing from a worker, whatever the guest's
access. So a write the window refuses is resolved as a read, and a load may
dirty a page in a writable mapping.

Handling KVM's faults needs `/dev/userfaultfd` access (see Testing).

### `vendor/libkrun`: the VMM

The VMM is a submodule: branch `fxdax` of
[semistrict/libkrun](https://github.com/semistrict/libkrun), whose `main` is
containers/libkrun plus HVF snapshot support. `fxdax` adds (decision-1 in the
backlog):

| Change | Why |
|---|---|
| The fs device is generalised over backends (`FsBackend`), and an fxdax backend, `devices/src/virtio/fs/fxdax.rs`, is added behind the `fxdax` feature (`make FXDAX=1`), with `krun_add_virtiofs_fxdax`, `krun_add_virtiofs_fxdax_volume` and `KRUN_FEATURE_FXDAX`. | The backend hands each descriptor chain to `fxdax::fuse::Server`, as volatile slices of guest memory. Devices serving volumes of one image share it, mounted once (`fxdax::Image`). An fxdax root cannot have libkrun's implicit init, as fxdax serves FUSE itself. |
| macOS: the VMM worker also starts for virtio-fs DAX windows. | It maps DAX windows into HVF. |
| macOS: the shm region is demand paged (`hvf::DemandPagedRegion`). Data and instruction aborts there go to `DaxWindow::fault`, and `HvfSecondStage`, an `fxdax::SecondStage`, mirrors the window into HVF with `hv_vm_map`, `hv_vm_protect` and `hv_vm_unmap`. | HVF tells the VMM where a fault happened. Pages the guest reads are also executable to it, so it can run programs straight from the share. |
| Linux: the filesystem takes over the shm region's host memory (`create_dax_window_in_host_memory`). | KVM follows it; see DAX on Linux above. |
| A fault the window can't resolve stops the guest on both hosts, rather than being treated as MMIO. | As in Zircon, where an unresolvable guest fault is fatal. |
| `libkrun.so` links with `-Bsymbolic-functions` when fxdax is enabled. | The in-process kernel defines the Zircon syscalls as C functions. On Linux, `libkrun.so` otherwise bound them to another copy in the process, such as the test runner's own fxdax. |
| `krun-hvf` builds on x86-64 Linux. | `make` builds every workspace member. |
| The test runner counts a timeout as a failure. | It used to report `OK - 0/0 passed`. |
| The guest tests `virtiofs-fxdax` and `virtiofs-fxdax-nodax`. | See Testing. |

## Not ported yet

The backlog (`backlog/`, managed with Backlog.md) tracks what remains. The
main gaps:

- **Upstream tests:** Fxfs's `file.rs` and `volume.rs` tests aren't ported
  yet (TASK-3).
- **Host mapping limit:** each mapped page is its own host mapping, so a large
  window can exceed `vm.max_map_count` (TASK-12).

## Testing

```sh
scripts/test.sh          # macOS or Linux
scripts/test-linux.sh    # Linux, in a Lima VM
scripts/test-gce.sh      # x86-64 Linux with KVM, on a disposable GCE host
```

The guest tests boot a libkrun guest with an fxdax share. They run file
workloads, including `mmap`, with DAX (`virtiofs-fxdax`) and without
(`virtiofs-fxdax-nodax`), and serve two volumes of one image as two devices
(`virtiofs-fxdax-volumes`). Afterwards the host remounts the image, fscks it,
and checks what the guest wrote.

```sh
scripts/test-e2e.sh        # macOS (HVF) or Linux (KVM)
scripts/test-e2e-linux.sh  # Linux, in a Lima VM
```

`scripts/test-gce.sh` runs them too, and copies the logs back. It creates a
labelled n2 host with nested KVM, which deletes itself after 8 hours, and
`all` always deletes it.

On macOS the guest tests need Homebrew's `llvm` and `libkrunfw`.

On Linux, DAX windows take KVM's faults through userfaultfd, which needs
access to `/dev/userfaultfd` (or `vm.unprivileged_userfaultfd=1`). To grant it
to the `kvm` group:

```sh
echo 'KERNEL=="userfaultfd", GROUP="kvm", MODE="0660"' |
    sudo tee /etc/udev/rules.d/99-userfaultfd.rules
sudo udevadm control --reload && sudo udevadm trigger --name-match=userfaultfd
```

## Updating from upstream

`scripts/upstream-changes.py REVISION` lists the files whose cited upstream
sources changed between the recorded revision and `REVISION` in a Fuchsia
checkout (`$FUCHSIA`, by default `~/src/fuchsia`); `--diff` prints the
changes. Bring them over, then record `REVISION` in `Cargo.toml`.
`--check` checks that every cited source exists at the recorded revision.

Formatting: run `rustfmt` on the files you change. `cargo fmt` would also
reformat the files copied from Fuchsia, which keep upstream's formatting.

## Licenses

- **The workspace:** BSD-3-Clause, as is Fuchsia (`LICENSE`, `PATENTS`,
  `AUTHORS`).
- **Ports of the Zircon kernel (`zircon`, `page`):** MIT, as Zircon is (their
  `LICENSE`).
- **libkrun:** Apache-2.0.
- **libkrunfw:** the guest kernel the tests load at runtime, GPL-2.0 and
  LGPL-2.1.
