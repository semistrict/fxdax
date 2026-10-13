How much stack Fxfs's operations use when one thread polls them all, as the guest kernel's threads
would (TASK-26.1): `cargo run --release -p fxdax --example fxfs-stack` at f46241c, on an Apple
silicon Mac (macos-arm64.md) and in its Lima VM, Ubuntu 24.04 (linux-arm64.md). Each figure is
the most a phase used below the executor's frames, which a kernel's executor would replace.

Linux gives kernel threads 16 KiB of stack on x86-64 and arm64, some of which the system call and
the VFS use before a filesystem is called.

- Metadata operations fit: create, mkdir, write, lookup, rename and unlink take 2-7 KiB.
- Formatting (34-35 KiB), mounting with replay (34 KiB) and journal compaction (25-28 KiB) do
  not.
- Unoptimized builds use 5-10 times as much (compaction 382 KiB), so only optimized builds say
  anything about a kernel's.

The largest frames, from `-Zemit-stack-sizes` on Linux arm64 (nightly 2026-10-12, release):

| Frame | Bytes |
|---|---:|
| `allocator::Flusher::finish` (async fn) | 13,872 |
| tokio's spawn of `FxFilesystem::trim_task` (the future, moved) | 13,216 |
| tokio's spawn of `Graveyard::reap_task` | 12,320 |
| `ObjectStore::flush_unlocked` (async fn) | 10,432 |
| `PersistentLayerWriter` (two functions) | 7,184-7,472 |
| `Journal::flush_task` (poll_fn) | 5,744 |
| `DataObjectHandle::overwrite` (async fn) | 3,888 |
| `Allocator::flush` (async fn) | 3,712 |

In Fxfs's own object, 24 frames are 4 KiB or more, 52 are 2 KiB or more, and 93 are 1 KiB or
more. Large async-fn poll frames hold the temporaries of every branch at once, and spawning a
future moves it through the stack.
