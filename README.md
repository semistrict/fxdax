# fxdax

Fuchsia's Fxfs filesystem, running in a virtual machine's own Linux kernel,
on extents its host keeps for it, so that the guest's file data is mapped
straight from the host's memory (DAX), one copy shared by every VM that maps
it.

The guest's kernel runs Fxfs (`guest/`), which keeps its filesystem as
extents: immutable pieces of data, written once and sealed by the next
commit (backlog doc-4). On the host, fxdax's extent daemon, `fxdax-extentd`,
keeps each VM's extents in an extent store on local disk, caches their pages
in physical memory of its own, and maps them into the VMs' DAX windows. Its
cache is shared by the host's VMs: two VMs mapping one extent, such as two
forks of one image, map the same pages.

## How it fits together

```
guest (Linux + Fxfs)     guest/: Fxfs in the kernel; DAX loads and stores
  │  virtio extent-store device: READ, WRITE, MAP, COMMIT...
Firecracker (one VM     vendor/firecracker: the extent device, a client of the
  process)               daemon
  │  Unix socket; the daemon's memory (a memfd), mapped into the DAX window
fxdax-extentd            crates/fxdax: extentd, the stores, the extent cache,
  │                      the DAX windows
physical memory          crates/physmem: arenas of shared memory (memfds),
  │                      their caps, relief and quarantine
extent store             crates/extent_store: segments, large extents' files,
                         an index
```

- **Requests:** the guest's Fxfs reads, writes, frees and commits extents
  through the device, which forwards each request to the daemon.
- **DAX:** `MAP` maps a range of an extent into the VM's DAX window, a range
  of guest physical memory the daemon keeps the page tables of: a sealed
  extent's pages from the cache, which reads them in from the store, an open
  extent's from the memory the guest writes it in, until a commit seals it.
- **Hypervisor mirror:** the daemon maps pages at the window's addresses, and
  the VM process maps the same pages of the daemon's physical memory (memfds)
  into the host memory KVM follows for the window, keeping it in step with
  userfaultfd.

## Where the code comes from

Code falls into three kinds:

- Fuchsia sources, copied;
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
- the SDK's `zx` crates, for their types: fxdax defines none of Zircon's
  syscalls, and neither Fxfs in the guest nor Fxfs on the host calls any.

Each crate's `Cargo.toml` cites the Fuchsia directory it is copied from. Most
files are as upstream wrote them. Those that aren't say how they deviate in
their leading comment:

- The runtime crates are built for the host only, without Fuchsia's runtime
  and handles.
- Code that needs Zircon, such as VMO-backed buffers, builds only when
  `target_os = "fuchsia"`: fxdax's buffers are plain memory. What can't build
  at all, such as FIDL protocols over channels or tests that need Fuchsia's
  component framework, is removed from those files.
- `storage_device` has no `FileBackedDevice`: fxdax has its own image device
  (see `crates/fxdax`).
- Fxfs can snapshot a volume (`RootVolume::snapshot_volume`), which upstream
  can't. The snapshot shares the source's layer files, by reference count,
  and its data extents. The allocator records a store's allocations under a
  data owner that a snapshot changes; the source's owner at the snapshot is
  frozen, its extents never deallocated one by one while any store shares
  them, and freed once none does. Blocks written in place (preallocated
  files) are written to new blocks where a snapshot shares them. fsck checks
  shared extents, and the on-disk format is version 59.
### `crates/physmem`: physical memory

The extent cache's pages, open extents', and the DAX windows', are host shared
memory: physical memory in arenas, each its own shared memory object (a memfd
on Linux, POSIX shared memory on macOS), which a process given an arena's
object maps the arena's pages from (TASK-43). An arena takes memory 2 MiB at a
time, and gives a chunk back once its pages are all free; arenas may be capped,
each and together (TASK-50.3); an arena starved at a cap asks for relief, which
evicts cached extents' pages; and a page a VM process's window may still map,
freed, waits in quarantine until the VM process has unmapped it (TASK-79.9).
It was the Zircon kernel port's PMM, fxdax's own code there, lifted out with
its tests when the port was deleted (TASK-96.1).

### `crates/fxfs-posix`: Fxfs's files and directories

One POSIX layer over Fxfs's object stores, which the guest kernel's VFS calls
(`guest/rust/fxfs-linux`) and the host's filesystem calls as a library
(`crates/fxdax`), generic over what differs between them (`Platform`): the
guest's, Fxfs in the kernel, the host's, images being built.

### `crates/`: stand-ins for Fuchsia-only pieces

Some crates only exist inside Fuchsia's build, or only work on Fuchsia. Each
of these stands in with the items the code copied from Fuchsia uses:

| Crate | Stands in for | How |
|---|---|---|
| `fidl` | FIDL runtime | No channels or wire format: the types the generated bindings use. |
| `fidl-fuchsia-io`, `-fs`, `-fxfs`, `-diagnostics-types` | generated FIDL bindings | Written to the shape of the generated types, for the types Fxfs uses. |
| `fuchsia-inspect`, `diagnostics-log` | diagnostics | No-ops, and stderr logging. |
| `bssl-crypto`, `mundane` | BoringSSL bindings | SHA-256/512 on the `sha2` crate. |
| `fxfs-unicode-gen` | build-time generated tables | Tables generated once from UCD 12.1.0 by Fxfs's own generator (`scripts/generate-unicode-tables.sh`). |

### `crates/fxdax`: fxdax's own code

`fxdax` holds the extent daemon and the host's side of Fxfs. Its crate docs
(`cargo doc -p fxdax --open`) are the guide.

- `extents.rs` holds `ExtentStore`, an extent store as a VM's device serves
  it: its I/O, and `MAP` and `UNMAP` into its DAX window.
- `cache.rs` holds the cache of sealed extents' pages, shared by every store
  in the process, each extent a table of its pages, read in a part at a time
  and evicted as memory runs out; and open extents' memory, which a commit
  moves into the cache uncopied.
- `extentd/` holds the daemon (`bin/fxdax-extentd.rs`) and its client,
  `RemoteExtentStore`, which a VMM's device is; see the extent daemon below.
- `dax.rs` holds DAX windows, their mappings and the pages they have mapped,
  and the `SecondStage` trait that follows a window's pages: the daemon's connection to a VM process implements it;
  `dax/uffd.rs` is the VM process's own second stage, in the host memory that
  KVM follows, described below.
- `filesystem.rs`, `volume.rs`, `image.rs` and `node.rs` hold the host's view
  of an Fxfs image: `Filesystem` and `Image`, which format, fill and check
  images, such as the stores a guest's Fxfs mounts, through `crates/fxfs-posix`
  as the guest's kernel does. A `Filesystem` is a volume whose nodes are `Node`
  handles, each holding its node, with the checks Linux's VFS makes for the
  guest; `volume.rs` keeps the nodes held, and runs the volume's one unmount.
  An `Image`'s volumes share its journal, space and executor.
- `import.rs` makes filesystems of container images (TASK-39.5): an OCI image
  layout's layers, or a `docker export` tarball, applied as the OCI image spec
  applies changesets (whiteouts, opaque directories), files keeping their
  owners, permissions, times and extended attributes. `fxdaxctl import` makes
  a base store of one, for VMs to boot forks of.
- `memory.rs` holds `VolatileSlice` and `VolatileSliceMut`, memory fxdax only
  copies into and out of, as a guest may change it concurrently.
- `error.rs` holds `Errno`, Linux's errno values on every host, mapped from
  Fxfs's errors, and `Error`, for setting up.
- `storage/` holds `Storage`: an image, or memory. It checks filesystems
  (fsck), and clones images: a clone is a fork of the image's extent store,
  whose directory is a handle naming the image's. `Storage::delete` deletes
  clones and images: an image whose clones live keeps its directory, holding
  what they share of it, until the last of them goes (TASK-39.4); removing an
  image's directory (`rm -rf`) while clones of it live is not supported: they
  hold their data in it.
  - `image.rs` is the device on images, in place of Fuchsia's
    `FileBackedDevice`: an extent store in a directory, `crates/extent_store`'s.
    Extents' I/O bypasses the host's page cache where the host filesystem
    allows (`O_DIRECT`, `F_NOCACHE`).
  - `io.rs` runs extents' I/O off Fxfs's executor threads: on an io_uring on
    Linux, and a small thread pool elsewhere.
- `host_store.rs` makes, opens and deletes both host stacks' extent stores in
  the host's directories, images (`Storage`) and guests' stores
  (`ExtentStore`), with one block size, one set of options and one way of
  minting branches, and cleans images' stores after their commits.
- `pages.rs` configures physical memory (`crates/physmem`): its size, which
  extents' pages come from, and tenants' arenas of it.
- `executor.rs` holds the fuchsia-async executor Fxfs runs on.

`crates/extent_store` is fxdax's too: backlog doc-4's extent store on local
disk, of segments, large extents' own files, and an index of checkpoints and
per-commit deltas, which recovers to a prefix of its commits after a crash and
cleans segments mostly freed: images' stores clean so after their commits,
the extent daemon's not yet. A store holds branches: a fork takes the same
time and space whatever the store holds, and shares its parent's files, which
go once no branch holds extents in them. Its tests run it on a file system in
memory that crashes at random points.

**DAX on Linux.** KVM maps a guest's memory from the host memory backing its
memory slot, and follows the host's mappings. On arm64, though, a fault it
can't resolve there is reported only as `EFAULT`, with no address. So that
memory must never fault that way. The VM process hands it to its store's
connection to the daemon (`create_dax_window_in_host_memory`), which keeps it
like this:

- Pages the window hasn't mapped are anonymous memory registered with
  userfaultfd for missing faults.
- Mapped pages are the daemon's pages mapped again, from its physical memory
  (memfds), at their offset there. They stay read-write, and userfaultfd
  write-protects them while the window maps them read-only.
- A fault thread in the VM process passes each event to the daemon, which
  resolves them concurrently, as futures, so a fault waiting for a page to be
  read in holds up no other.
- Changing a page's mapping replaces its memory in several system calls. Until
  userfaultfd tracks the new memory, it is inaccessible, or read-only where a
  store would go untracked, so a fault in between fails with `EFAULT` rather
  than resolving wrongly, and the VMM retries the vCPU.

x86 KVM faults pages in for writing from a worker, whatever the guest's
access. So a write the window refuses is resolved as a read, and a load may
dirty a page in a writable mapping.

KVM would map each page for the guest on a fault of its own, which nested
virtualization makes slow. Mapping an extent maps the pages it has in memory
(an open extent's all of them, committed as they are mapped), and the window
tells the VMM of the pages it maps (`Prefault`), which the VMM has KVM map
ahead of the guest (`KVM_PRE_FAULT_MEMORY`).

Handling KVM's faults needs `/dev/userfaultfd` access (see Testing). The host's
kernel must be Linux 6.1 or later: `/dev/userfaultfd` (6.1), userfaultfd's
write protection of shared memory (`UFFD_FEATURE_WP_HUGETLBFS_SHMEM`, 5.19) and
`MADV_POPULATE_WRITE` (5.14). In a jail, whose VMM has no privileges left,
the device node must be in the jail, rather than `vm.unprivileged_userfaultfd=1`
set, which every process on the host would get (TASK-50.4). Firecracker's
jailer makes it there when the host has it, owned by the VMM's uid and gid,
mode 0600; a jail made otherwise needs the same (`mknod dev/userfaultfd c 10
MINOR`, its minor as the host's `/dev/userfaultfd` has it).

**The extent daemon.** The VMM runs one VM per process, so extents cached in a
VM process would be shared by that VM alone. `fxdax-extentd` (the `extentd`
module) is one process on a host that owns the physical memory, the extent
cache and the stores, and serves them to VM processes over a Unix socket: the
only way the VMM's extent device has a store, at the socket its configuration
names (`extentd_socket`). Physical memory is memfds, which each VM process
receives once: its
window's pages are pages of it, which the VM process maps into its own memory
at the offsets the daemon names, so two VMs mapping one extent map the same
pages. The daemon's changes to a window reach the VM process in order before
the replies after them, unmapping and write-protecting acknowledged before the
daemon frees or shares a page; guests' faults go from the VM process's
userfaultfd to the daemon. One daemon serves every tenant on a host. Guest
isolation is the security boundary: a guest reaches only what its window maps,
so one tenant for everything is secure. As defence in depth against a VM
process escaped from its guest, physical memory is in arenas, each
its own memfd (TASK-43): a tenant's stores' pages are of its arena, and public
bases' pages, which every tenant's forks share, one copy, of the default arena.
A tenant's VM process holds its tenant's arena and the default one, read-only
(mapped private, so the window can track stores to it), and nothing of another
tenant's. An extent's pages are of the arena it is cached in alone: a commit
moves an open extent's pages into its cached extent only within one arena.
The host's VM manager drives the daemon
through a gRPC control API on a Unix socket of its own (`FXDAX_EXTENTD_GRPC`;
`crates/fxdax/proto/fxdax/extentd/v1/extentd.proto`, TASK-43.3): it makes,
imports, forks, places, syncs, resizes and deletes stores, and grants tenants
(TASK-43.4). Grant returns a tenant's own VM socket, for the VM manager to
bind-mount into the tenant's jails; a connection's tenant is the socket it came
in on, and it names stores in the tenant's directory alone. Revoked, a
tenant's socket goes, and once its VM processes do, its arena and cached pages.
The daemon's own VM socket is the default tenant's: its connections name
stores in the daemon's roots alone (`FXDAX_EXTENTD_ROOTS`), or anywhere without
any. A VM process's user does not matter: nothing below a tenant is separated.
The daemon must be able to kill each VM process (it is the daemon's user's, or
the daemon has `CAP_KILL`), and refuses a connection whose process it could
not kill: one that stops acknowledging the daemon's changes to its window,
frozen, holds the pages it may still map until the daemon has killed it
(TASK-79.9). It knows the process by the socket's pidfd (`SO_PEERPIDFD`, Linux
6.5), or, on an older kernel, by the pid of the process that connected
(`pidfd_open`).

**The daemon's memory.** DAX moves the guests' file pages out of their RAM into
the daemon's (TASK-50.3). Its physical memory is at most
`MEMORY_BYTES` (its second argument, 1 GiB by default), taken by arenas in
2 MiB chunks as they need them. The control API limits, at runtime, the memory
of each tenant and of every tenant together (`SetMemoryLimit`): past a
tenant's limit, the daemon evicts the clean pages of its extents read longest
ago; past the total, or out of `MEMORY_BYTES`, any tenant's. With none left to
evict, what would take more fails with `ENOMEM`, which the guest sees as an I/O
error, and the daemon goes on. `ShrinkMemory` evicts toward a target at once,
as the host's memory runs low. `/metrics` shows the memory held, every
tenant's together and each tenant's, by the first 16 hex digits of its name's
SHA-256 (`printf %s NAME | sha256sum | cut -c1-16`), not its name. Under cgroup
v2 the arenas' memfds are shmem, charged to the cgroup of the process that
first touches each page, and uncharged when the chunk goes back (a hole
punched): that is the daemon, which faults each chunk in as an arena takes it
and writes pages before mapping them to a VM. So the daemon's cgroup is charged
for every VM's cached file pages, and the VMs' cgroups are not: give the
daemon a cgroup of its own whose `memory.max` holds `MEMORY_BYTES` and its heap
(the stores' metadata, the connections' buffers), and limit tenants through the control API, as cgroups cannot see them apart. A
memory.max under that is met by the cgroup's OOM killer, not by eviction:
lower `MEMORY_BYTES` or the total limit instead.

**Multi-node only.** A branch's durable copy is in a bucket, and hosts hold
copies of it, whose placements fence each other with their tokens: a placement
cut off neither takes a branch back nor deletes it from under a later one
(TASK-39.7, TASK-45). The daemon runs as a host of that system, with the bucket
(`FXDAX_EXTENTD_REMOTE`) and its placement's token from the control plane
(`FXDAX_EXTENTD_TOKEN`, not 0), and refuses to start without them: there is no
single-node mode. Tests and development use an in-memory bucket
(`memory:///NAME`). fxdaxctl, without the daemon, changes existing stores'
branches on the same terms, and makes new stores (`take`, `import`) anywhere.

### `vendor/firecracker`: the VMM

The VMM is a submodule: branch `fxdax-v1.16.1` of
[semistrict/firecracker](https://github.com/semistrict/firecracker), which is
Firecracker's v1.16.1 release, the sandboxes' pin, plus fxdax's changes, a
commit each, built with its `fxdax` feature (TASK-39.6). Branch `fxdax`, on
Firecracker's main branch, where they began, is superseded by it: it stops at
TASK-37's change, and takes no more. fxdax ran on
libkrun before; it left it for Firecracker, which the sandboxes it serves run
(TASK-40).

| Change | Why |
|---|---|
| The extent-store device (`devices/virtio/extent`), behind the `fxdax` feature: `PUT /extent/{id}` before boot, or the configuration file's `extent` list, names its tag, store, capacity, DAX window and the extent daemon's socket (`extentd_socket`). Its store is served by the daemon (`fxdax::RemoteExtentStore`). | The guest's own Fxfs keeps its filesystem in it (backlog doc-4). No fxdax code serves a store in the VM process: the daemon owns the stores and their cache, which the host's VMs share. |
| Virtio shared memory regions on the MMIO transport, and `KvmMemSlot`, a KVM memory slot any device may map host memory with. | The device's DAX window is a shared memory region, the daemon's memory mapped in; KVM follows it. |
| The window maps the pages the daemon maps for the guest ahead of its accesses (`KVM_PRE_FAULT_MEMORY`). | Each of KVM's own faults on a page costs an exit, slow under nested virtualization (TASK-25.3). |
| A vCPU retries a `KVM_RUN` failing with `EFAULT` while a window is remapped. | The daemon's changes to a window's host memory make the guest's access fail meanwhile; see DAX on Linux above (TASK-25.4). |
| `WRITE_COMMIT`, INFO's `sequence_floor`, and the store's capacity in the device's configuration, changed while the guest runs. | doc-4's requests (TASK-36, TASK-37). |
| Snapshots of a VM with the extent device: `PUT /snapshot/create` forks the store to `<snapshot>.extent-<id>` and saves the window's mappings; `PUT /snapshot/load`, with `extent_overrides` naming each restored VM's own store, forks it from that point and maps the window again. The guest's filesystem must be frozen (fsfreeze) as the snapshot is taken, and thawed after a restore, when it learns its store's branch (doc-4, Snapshots). | A zygote's children, suspend-to-RAM and moving VMs, each restored VM with a store of its own (TASK-39.1). |

### `guest/`: Fxfs in the guest's kernel

The design in backlog doc-2 moves Fxfs into the guest's Linux kernel, over a
virtio extent store. Its first pieces, from TASK-26.1's spike:

- `guest/linux/fs/fxfs`: the kernel's side, in C. `super.c` is the VFS glue
  (superblock, inodes, lookup, readdir, read) over the filesystem's Rust
  (`fxfs.h`), and `rt.c` gives Rust memory (kvmalloc), logging, panics,
  sleeping, work items, timers, the clocks and randomness (`rt.h`). Every call
  into Rust runs in a NOFS allocation scope.
- `guest/rust`: Rust built for the kernel's targets
  (`aarch64-unknown-none-softfloat`, `x86_64-unknown-none`), a workspace of
  its own on a pinned nightly. `scripts/build-guest-sysroot.sh` builds `std`
  with `core` and `alloc` (`-Zbuild-std`) into a sysroot, which the rest of
  the kernel's Rust, Fxfs included, builds against. `linux-rt` is the
  runtime: the global allocator, printk logging, the panic handler, and an
  executor (`block_on` sleeps the calling thread, `spawn` polls on kernel
  workers, `Timer` uses hrtimers).
  `fxfs-linux` is the filesystem, Fxfs for the kernel, and `std` is a std for
  crates written for one (see below).
- `scripts/build-guest-kernel.sh` builds Amazon Linux's 6.18 kernel (the
  release it pins), configured as Firecracker's CI configures the 6.18 guests
  it supports, with fs/fxfs built in, the Rust linked into it as one object of
  what the kernel calls (link-time optimized). The kernel itself has no Rust
  support enabled: Kbuild never sees the Rust. fxdax's changes to the kernel
  are a series of four commits on the release (`guest/linux/patches`, each
  with the files of `guest/linux` it adds), which the script makes in the
  kernel's tree and builds. Its `out/<arch>` has what builds the same kernel
  elsewhere: `patches/` (`git am` them on the release), `libfxfs_linux.a`
  (copy it to `fs/fxfs/`) and `fxdax.config` (fxdax's options, over
  Firecracker's configuration).
- `scripts/probe-guest-std.sh` builds Fxfs's third-party dependencies
  (`guest/std-probe`) for the kernel's targets, against a sysroot whose `std`
  is `guest/rust/std`, and lists those that do not build.

**Guest Fxfs: what it does not do** (TASK-26.19). Beside the core inode, file
and superblock operations, it has O_TMPFILE, fallocate (allocate, punch hole,
zero range, keep size), fiemap with SEEK_DATA and SEEK_HOLE, POSIX ACLs,
extended attributes, chattr's immutable, append-only, no-dump and no-atime
flags (kept in its mode's bits above the kernel's 16, which Fxfs's objects have
room for), idmapped mounts, multigrain timestamps, O_DIRECT (through Fxfs, past the page
cache, aligned or not), reflinks (`FICLONE`, `FICLONERANGE`, `cp --reflink`,
and `copy_file_range`, which share extents, counted per range in the
allocator, and copy nothing), freezing, NFS export's file handles, and a
shrinker of its cached records. What programs see of the rest (the guest
suites' `unsupported` check pins the first two):

- Deduplication (`FIDEDUPERANGE`, `duperemove`) reports `EOPNOTSUPP` in each
  destination's status.
- Extents written in place (Fxfs's overwrite modes, which nothing in the guest
  makes) can't be shared: a reflink of them fails with `EOPNOTSUPP`.
- `MAP_SYNC` mmaps fail with `EOPNOTSUPP`: the DAX window is the host's
  memory, not persistent memory, and what is stored through it is durable once
  an fsync commits it, never at the fault.
- chattr's other flags (`FS_IOC_FSSETXATTR`'s project IDs and extent sizes,
  synchronous, compressed, no-COW) fail with `EOPNOTSUPP`.
- No change counter (`i_version`): the change attribute, an NFS server's or
  IMA's, is the ctime, which is fine-grained (`FS_MGTIME`) once read.
- Fxfs's in-memory layers of its trees, bounded by its journal's size, are not
  shrunk under the guest's memory pressure; its cached records and its page
  cache are.
- Files not on DAX are cached in 4 KiB folios, not large ones: more overhead
  per byte in large sequential I/O (TASK-26.19.11).

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

`scripts/lima-setup.sh` creates the Lima VM the Linux scripts use on macOS, and
installs what they need in it; run again, it installs only what is missing.

The guest tests (`guest/tests`) boot fxdax's guest kernel in fxdax's
Firecracker, on Linux with KVM, and mount its Fxfs from an extent-store device,
whose store fxdax's extent daemon serves (`scripts/test-e2e.sh` starts one):
`extent-guest` (the driver), `fxfs-guest`, `fxfs-guest-dax`, `fxfs-guest-checks`
and `fxfs-guest-checks-dax` (file workloads, including `mmap`, after which the
host remounts the store, fscks it, and checks what the guest wrote), the crash
tests, resizing a mounted store, and pjdfstest. The harness (runner, guest agent
and test cases) came from libkrun's tests, Apache-2.0 (`guest/tests/LICENSE.libkrun`).

```sh
scripts/test-e2e.sh        # Linux (KVM): FXDAX_GUEST_KERNEL names the kernel
scripts/test-e2e-linux.sh  # Linux, in a Lima VM
```

`scripts/test-guest-kernel.sh` builds the guest kernel with Fxfs in it and runs
the guest tests on it, in Lima on macOS.

`scripts/test-gce.sh` runs them too, and copies the logs back. It creates a
labelled n2 host with nested KVM, which deletes itself after 8 hours, and
`all` always deletes it (`scripts/lib/gce.sh`).

The protocols whose bugs are rare interleavings have TLA+ specs, which TLC
model-checks exhaustively over small bounds (`specs/`): live read-only
followers (`specs/follower`, backlog doc-8) and the guest Fxfs's DAX seal and
relocation protocol (`specs/dax`, `guest/linux/fs/fxfs/dax.c`). Each rule of a
protocol has a mutation, the spec with that rule removed, which TLC must find
the violation of: `specs/dax`'s include dce4f14's bug, which TLC finds as the
guest's seal_race did.

```sh
make check-tla             # a few minutes, on Java; fetches TLC once
```

## Benchmarks

`scripts/bench.sh` compares the guest kernel's Fxfs, on an extent-store device
the extent daemon serves, with a DAX window and without, with ext4 on the same
kernel, over a virtio-blk disk backed by an image file on the same host
filesystem and with DAX on writable virtio-pmem. The workloads are fio's sequential and random reads and
writes (cold and warm), fsync-heavy appends and I/O through `mmap`, creating,
stating, listing and removing 10,000 files, and untarring and building GNU
make (`guest/tests/test_cases/src/test_fs_bench.rs`).
`scripts/bench-report.py` turns the measurements into a table of medians,
spreads and ratios to ext4. The guests' root filesystem is a Fedora image built
with podman.

```sh
scripts/bench-gce.sh       # x86-64 Linux with KVM, on a disposable GCE host
scripts/bench.sh           # this host, Linux
scripts/bench-linux.sh     # Linux, in a Lima VM
```

`scripts/bench-gce.sh` keeps its results in `docs/measurements`, with the host
and the source it measured.

For SQLite, set `FXDAX_BENCH_GROUPS=sqlite`. This compares cold and warm scans
through pread and mmap, single-update commits and batches of 100 updates in
DELETE/FULL, WAL/FULL and WAL/NORMAL modes, and explicit WAL checkpoints.
`FXDAX_BENCH_SQLITE_TXNS` sets the commits per workload (default 200).
The group also checks database integrity after reopening, recovery from a
killed writer, and a WAL reader's snapshot during a concurrent writer's commit.
Process recovery does not test power-loss durability. WAL/NORMAL commit rates
exclude the separately reported checkpoint and have different durability
semantics from WAL/FULL.

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
  `AUTHORS`). What was copied or adapted from Fuchsia keeps The Fuchsia
  Authors' copyright; fxdax's own code, and its changes, are The fxdax
  Authors'.
- **`physmem`:** MIT (its `LICENSE`), as the Zircon kernel port it was lifted
  from was (deleted, TASK-96.1), keeping the copyrights of the sources it
  ports.
- **The parts of Rust's std imported into the guest's
  (`scripts/import-guest-std.py`):** MIT OR Apache-2.0, as Rust is (the
  licenses and Rust's `COPYRIGHT` in `guest/rust/std`).
- **`guest/linux`:** GPL-2.0 or BSD-3-Clause, built into the guest's Linux
  kernel (GPL-2.0), which `scripts/build-guest-kernel.sh` downloads; its
  patches to the kernel are GPL-2.0, as the kernel is.
- **The Unicode tables (`fxfs-unicode-gen`):** generated from the Unicode
  Character Database, under the Unicode license (`LICENSE-UNICODE`).
- **Firecracker (`vendor/firecracker`):** Apache-2.0.
- **The guest tests' harness (`guest/tests`):** libkrun's test framework,
  Apache-2.0 (`guest/tests/LICENSE.libkrun`), its files keeping libkrun's
  license; the test cases fxdax wrote are its own, BSD-3-Clause.
- **Amazon Linux's kernel:** whose sources `scripts/build-guest-kernel.sh`
  builds the guest kernel from, GPL-2.0, with Firecracker's configuration of
  them (`guest/linux/configs`, from Firecracker's main branch: Apache-2.0).

Each source file's header says which applies to it: `make
check-license-headers` checks that each has the header its origin needs
(`scripts/license-headers.py`), and `make fix-license-headers` adds those
missing.
