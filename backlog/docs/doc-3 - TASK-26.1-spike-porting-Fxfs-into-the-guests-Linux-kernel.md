---
id: doc-3
title: 'TASK-26.1 spike: porting Fxfs into the guest''s Linux kernel'
type: other
created_date: '2026-10-13 21:31'
updated_date: '2026-10-13 21:31'
---
# TASK-26.1: porting Fxfs into the guest's Linux kernel

Spike report, 2026-10-13. It sizes porting Fxfs's code into the guest kernel (doc-2, requirement
5), and recommends how.

**Recommendation: go.** A prototype is built into the guest kernel and mounted in a libkrun guest.
It is Rust built outside the kernel's build, with C VFS glue, an executor on kernel workers and
timers, and the kernel's allocator. Nothing found blocks the port, but Fxfs's stack use has to be
cut, and the guest's kernel stacks enlarged. Porting Fxfs onto that base, without the extent-store
device, is about 9-14 engineer-weeks; the estimate is below.

## What was built

| Piece | Where | Lines |
|---|---|---:|
| VFS glue: mount, inodes, lookup, readdir, read | `guest/linux/fs/fxfs/super.c` | 229 |
| What the kernel gives Rust: kvmalloc, printk, panic, completions, work items, hrtimers, clocks, randomness | `guest/linux/fs/fxfs/rt.c`, `rt.h` | 210 |
| Rust runtime: global allocator, logger, panic handler, `block_on`, `spawn`, `Timer` | `guest/rust/linux-rt` | 360 |
| The filesystem, a stand-in: one file whose contents a spawned task returns after a timer | `guest/rust/fxfs-linux` | 187 |
| std for the kernel, for crates written for std | `guest/rust/std` | 2,200 |
| Kernel build: libkrunfw's, with fs/fxfs and the Rust built in | `scripts/build-guest-kernel.sh` | |
| Guest test: boots that kernel, mounts fxfs, lists and reads it | libkrun `fxfs-guest`, `scripts/test-guest-kernel.sh` | |
| Fxfs's stack use, by operation | `crates/fxdax/examples/fxfs-stack.rs`, docs/measurements/fxfs-stack-2026-10-13 | |
| Fxfs's third-party crates against the kernel's std | `guest/std-probe`, `scripts/probe-guest-std.sh` | |

`fxfs-guest` passes on macOS (HVF, arm64). The kernel also builds for x86-64, with the Rust
object linked; booting it on x86-64 KVM (GCE) is left to milestone 1. The Rust adds 14 KB of
text.

## The kernel to target

**libkrunfw's: 6.12 LTS, as libkrunfw v5.6.2 patches and configures it (6.12.109).** libkrun
depends on libkrunfw's patches: transparent socket impersonation, vsock datagrams, DAX with
blocks smaller than pages, and FUSE DAX fixes. The glue is small C over stable VFS interfaces,
so following libkrunfw to its next LTS is cheap.

The approach below does not use the kernel's Rust support, so its state does not constrain the
choice. Upstream has no Rust VFS bindings even in 7.3-rc3: `rust/kernel/fs` has only `File` and
`Kiocb`. Wedson Almeida Filho's VFS series was never merged. There are also no virtio bindings
and no async executor. libkrunfw's config has `CONFIG_RUST` and `CONFIG_MODULES` off.

## How Rust gets into the kernel

**Cargo builds the Rust, and Kbuild links the result.** The kernel builds Rust itself, crate by
crate, without cargo. That cannot build Fxfs's tree: about 60 crates from crates.io, 9 of them
proc macros. So:

- `guest/rust` is a cargo workspace for the kernel's targets: `aarch64-unknown-none-softfloat`,
  and `x86_64-unknown-none` with the kernel code model, a static relocation model and return and
  retpoline thunks. It is built with no unwind tables and no function sections, as the kernel's
  own Rust is. It builds `core` and `alloc` (`-Zbuild-std`) on a pinned nightly.
- Release builds are LTO'd into one object of what the exported functions reach. Compiler
  intrinsics are an object apiece, so the kernel links only those used. Their weak `memcpy`,
  `memset` and `memcmp` lose to the kernel's.
- fs/fxfs's Makefile partially links that object (`ld -r -u fxfs_init`), strips its LLVM bitcode,
  and builds it into fs/fxfs. The kernel links with no warnings.
- The VFS glue is C: it calls the kernel's inline functions and macros directly, and keeps the
  Rust boundary to a FUSE-like set of calls (`fxfs.h`).

Still to do: localize the Rust symbols (about 380 `_R…` globals land in vmlinux), and keep
`CONFIG_RUST` off or rename the allocator's `__rust_*` symbols.

## Async in the kernel

Fxfs is async throughout, and uses fuchsia-async for:

- `Task::spawn` (29 uses);
- `Timer` (26);
- `MonotonicInstant` (18);
- `LowPriorityTask` (5);
- `Scope` (4);
- `yield_now` (1).

The prototype's executor covers all of them.

- **`block_on`:** a VFS operation polls its future on the calling thread, which sleeps on a
  completion between polls. Wakers complete it, which is safe from any context. The future is
  boxed, because kernel stacks are small.
- **`spawn`:** background tasks (`async-task`) run a poll per wake on `system_unbound_wq`.
  Scheduling allocates a work item with `GFP_ATOMIC`, because a timer's softirq wakes tasks. The
  port should embed one work item per task instead.
- **`Timer`:** hrtimers in softirq mode, waking through an `AtomicWaker`.

A guest `fuchsia-async` over linux-rt, with the API above, is about 400 lines. `LowPriorityTask`
maps to a low-priority workqueue.

## Allocation

Rust's global allocator is kvmalloc(`GFP_KERNEL`): slabs, falling back to vmalloc, with
alignment up to a page. Fxfs allocates large buffers and is not written to survive allocation
failure, so a failure panics the kernel, as one does in Rust. Two rules keep this safe:

- Every call into Rust, and every task poll, runs in a NOFS scope (`memalloc_nofs_save`), so
  reclaim never recurses into a filesystem.
- Code that can run where sleeping is not allowed does not allocate: wakers and timer callbacks.

## Stack: the main risk

Kernel threads have 16 KiB of stack on x86-64 and arm64. The system call and the VFS have used
some of it before a filesystem is called. `fxfs-stack` measures each operation's stack below
the executor, with one thread polling everything, in optimized builds. The numbers are from
docs/measurements/fxfs-stack-2026-10-13; macOS and Linux arm64 agree within 3 KiB.

| Operation | KiB |
|---|---:|
| lookup | 2 |
| create, mkdir, rename, unlink, read, write | 5-7 |
| fsck | 8 |
| journal compaction | 25-28 |
| mount with journal replay | 34 |
| format | 34-35 |

Metadata operations fit. Compaction, mount and format do not.

The cause is a few large frames. Async-fn poll frames hold every branch's temporaries at once,
and spawning moves a whole future through the stack:

| Frame | Size |
|---|---:|
| `Flusher::finish` | 13.9 KB |
| spawning `trim_task` | 13.2 KB |
| spawning `reap_task` | 12.3 KB |
| `ObjectStore::flush_unlocked` | 10.4 KB |
| `PersistentLayerWriter` | 7.2 KB |
| `Journal::flush_task` | 5.7 KB |

24 of Fxfs's frames are 4 KiB or more. Debug builds use 5-10 times as much, so the guest's Fxfs
must always be built optimized.

Plan:

1. Build the guest kernel with 32 KiB stacks. This is a one-line patch (`THREAD_SIZE_ORDER` on
   x86-64, `MIN_THREAD_SHIFT` on arm64), and costs 16 KiB per thread.
2. Box the largest futures in Fxfs, and have `spawn` build tasks on the heap, until mount,
   replay and compaction fit in about 24 KiB. That is about 10 small deviations from upstream.
3. Keep `fxfs-stack` as the guard, with a limit, and check the kernel's own stack use
   (`CONFIG_DEBUG_STACK_USAGE`) in the guest tests.

Running Fxfs on stacks of its own, by switching stacks, would also work, but it breaks the
kernel's unwinders and stack checks. It is not recommended.

## Dependencies

### std

Rather than rewrite Fxfs and its dependencies for `no_std`, the kernel gets a `std` of its own.
`guest/rust/std` is `core` and `alloc` under std's paths, plus what the kernel can back:

- locks, `Once`, `OnceLock` and `LazyLock`;
- `io`'s traits and in-memory readers and writers;
- the clocks, randomness, and hash maps with std's hasher.

A sysroot that holds it as `std` makes crates written for std build unchanged. Fxfs itself uses
nothing std-only outside its tests beyond collections, locks, time and `io`, so it should build
unchanged too.

Against that std, with fxdax's features, 44 of Fxfs's 61 third-party crates for the target build
unchanged (`scripts/probe-guest-std.sh`). Seven fail, and about ten more wait on them:

| Crates | Need | Fix |
|---|---|---|
| `log`, `rand` | Hash maps called by path (`HashMap::get(…)`), which a `Deref` wrapper cannot serve | Use std's own hash map wrappers, from rust-src (MIT/Apache) |
| `event-listener`, `rand`, `rapidhash` | Per-thread state in `thread_local!`: a cached parker, a thread RNG, a hash seed | A thread-local slot in the guest kernel's tasks (a small kernel patch), or those crates' features without it |
| `num-traits` | std's float math | Its `libm` feature instead of `std` |
| `serde_core`, `futures-io` | Gaps: `Path` conversions, vectored I/O | Add them |
| `getrandom` | A backend | Its custom backend over `get_random_bytes` |

The prototype's locks, condition variable and parking spin. The port backs them with the
kernel's mutexes, wait queues and completions.

### Fuchsia's crates

| Crate | Lines | What Fxfs uses | In the guest |
|---|---:|---|---|
| fuchsia-async | 3,436 (on tokio) | spawn, timers, instants, scopes | A guest version over linux-rt (~400 lines) |
| zx, zx-types, zircon | 51,937 (the in-process kernel) | `Status`, monotonic time, one `EventPair`, one `NullableHandle` | A guest `zx` of those (~400 lines) |
| fuchsia-sync | 389 (parking_lot, which parks threads with futexes) | `Mutex`, `RwLock`, `MappedMutexGuard` | `lock_api` over the kernel's locks (~150 lines) |
| fidl-fuchsia-io, -fxfs | stand-ins on the FIDL runtime | ~10 types (`MutableNodeAttributes`, `HashAlgorithm`, …) | Types only (~300 lines) |
| storage_device | 2,070 | `Device`, buffers on VMOs | Buffers in kernel memory; the device becomes the extent-store client (TASK-26.3, 26.5) |
| fuchsia-inspect, fuchsia-emulated-handle | 210, small | Metrics no-ops; a wake-lease type | Build as they are |

The rest of Fuchsia's crates are plain Rust and should build against the kernel's std:

- `fxfs-crypto` (3,192 lines);
- `fuchsia_merkle` (1,220);
- `fsverity_merkle` (854);
- `storage_ptr_slice` (799);
- `storage_units` (500);
- `fxfs-unicode` (426);
- `fscrypt` (385);
- `fprint`;
- `fuchsia-hash`.

## Estimate

For Fxfs mounted in the guest kernel, without the extent-store device (TASK-26.2, 26.4 and 26.5)
and without DAX:

| Work | Engineer-weeks |
|---|---:|
| The kernel's std, finished: the kernel's locks and parking, thread-locals, std's hash maps, the gaps | 1-2 |
| Fuchsia's crates for the guest: fuchsia-async, zx, fuchsia-sync, FIDL types, storage_device's buffers | 1-2 |
| Fxfs and its tree built for the kernel, and the fallout | 1 |
| Stack: 32 KiB stacks, boxed futures, checks | 1 |
| VFS glue for POSIX: an inode cache, create, mkdir, unlink, rename, link, symlink, setattr, xattrs, the page cache and writeback, fsync, statfs, mount options | 4-6 |
| Running it on x86-64 KVM, and in the guest tests | 1-2 |
| **Total** | **9-14** |

The VFS glue is the largest part, and DAX (TASK-26.6) adds to it.

## Risks found

- **Stack:** as above. The guest's Fxfs must always be optimized, and its stack use watched.
- **Panics:** Fxfs's assertions, and allocation failures, panic the guest's kernel. On Fuchsia
  they crash a component, which restarts. Release builds without debug assertions limit this.
- **Nightly Rust:** the build needs `-Zbuild-std`, as the kernel's own Rust needs unstable
  features. The toolchain is pinned in `guest/rust/rust-toolchain.toml`.
- **Symbols:** Rust's symbols are global in vmlinux until localized.
