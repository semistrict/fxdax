---
id: doc-6
title: Delayed metadata in the guest's Fxfs
type: other
created_date: '2026-10-15 08:43'
updated_date: '2026-10-15 08:43'
---
# Delayed metadata in the guest's Fxfs (TASK-26.18.2)

## Why

The unlink family is 2-5x slower than ext4 on DAX pmem. That is unlink, unlink-parallel, unlink-shared-dir and large-dir-unlink.

The VFS calls `->unlink` holding the directory's i_rwsem. Fxfs's unlink does all of its metadata work before it returns:
- it builds a transaction, looking records up;
- it takes the global commit mutex;
- it serializes the journal record;
- it applies about 4 mutations to the in-memory skip list and the TreeCache.

The other threads spin on the rwsem meanwhile. With 2 vCPUs that is half the machine.

XFS keeps the same lock held but does almost nothing under it (doc-5). btrfs takes the B-tree work out of the syscall with delayed items. Fxfs's per-op tree cost is like btrfs's, so it needs the same: the syscall records the change, and a worker applies it.

## Design

**What is delayed.** First unlink (and rmdir of empty directories), and the evict that tombstones an unlinked inode. Later, if they pay: create, link, rename, setattr.

**The queue.** One per filesystem, in Rust:
- a FIFO of operations, each with the inodes it touches (directory, child);
- a per-inode count of pending operations;
- one worker task that takes operations in order.

The worker groups consecutive operations, up to N (32), into one Fxfs transaction where their locks allow. Fxfs then journals and applies them together, as the XFS CIL aggregates. Group commits reduce the per-op journal-record and commit-mutex overhead.

**Syscall path.** `fxfs_unlink` checks only what Fxfs alone knows can fail, then queues and returns. The kernel already checked existence, type, emptiness of directories (through Fxfs's lookups) and permissions. The directory's attributes (mtime, ctime, size) are computed by the VFS glue, as for any change, and carried in the operation.

**Reads see pending changes by draining first.** Every entry point that reads Fxfs state an operation may change waits until the pending operations on that inode are applied:
- lookup of a name in a directory with pending operations;
- readdir;
- getattr from Fxfs;
- create, link or rename in the directory;
- fsync of the directory or the child;
- `sync_fs`, which drains all.

The dcache already answers most lookups without Fxfs. Unlinked names are negative dentries.

**Ordering and crash consistency.**
- Operations are applied in syscall order, so the journal order is the syscall order and the journal stays a valid prefix after a crash.
- A crash loses the queued operations along with the rest of the unflushed journal, which doc-2's requirement 2 allows.
- fsync and sync drain first, so they keep their promise.

**Errors.** An operation that fails in the worker can't fail its syscall. The filesystem logs it and goes read-only (as ext4's `errors=remount-ro`). Unlinks borrow metadata space, so ENOSPC can't happen; only I/O or corruption errors can.

**Eviction.** Evict of an unlinked inode queues its tombstone behind the unlink (FIFO), not on the graveyard directly.

**Memory.** The queue is bounded (e.g. 4096 operations). A syscall blocks when it is full, which gives backpressure as XFS's inodegc does.

## Tests

- **Guest suites unchanged:** fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax and pjdfstest.
- **New guest checks:**
  - readdir right after unlink doesn't list the name;
  - unlink then create of the same name works;
  - an unlink then fsync of the directory survives a crash;
  - parallel unlinks and creates in one directory end with exactly the names expected.
- **Crash test:** power-off with queued unlinks recovers to a prefix.

## Expected effect

- The time under i_rwsem drops to queueing (about 1 us).
- The worker's batched commits set unlink throughput, about 1/(per-op apply cost) at roughly 4-5 us.
- unlink-shared-dir and unlink-parallel are bounded by one worker CPU, not by rwsem spinning, so they should approach single-thread unlink rates or better.
