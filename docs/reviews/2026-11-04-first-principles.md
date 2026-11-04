# First-principles design review, 2026-11-04

Six read-only reviews of origin/main (cfd41e1a0), each of one area from what it must guarantee
rather than from the code: guest DAX; the host storage layers; extentd and the VMM; Fxfs on
write-once extents; the architecture against its goals; and the bug history as evidence. Ranked
by value. Decisions they need are at the end.

## 1. Writable DAX on write-once extents (four reviews of six, independently)

A window page *is* the open extent's block, and every fsync seals every open extent, so the next
store to any page of any file moves its block, in the fault path, racing the next seal.

- **Cost:** DAX runs at 0.51-0.61x no-DAX under pgbench (gate r261, r264; sandbox round 9: 1,012
  against 1,736 tps), and below stock ext4 dax=always on virtio-pmem (1,125-1,197). About 25k
  lines (dax.c 5k, guest/sim 9k, window/uffd/physmem about 10k), 7 patches to fs/dax.c, 3 of 4
  TLA+ specs, about 47% of bug-titled tasks, 37-39 of 65 dead-ends.
- **Bug rate not falling:** fafefbac7 ended evacuation's class (13 bugs, then 0), TASK-153.3 the
  open-state class, TASK-153.5 write(2)'s zeros; guest/sim still found four new fault-path bugs on
  11-03 (0e150b315, c40771b95, e8250a479), and dax.c grew from 4,561 back to 4,983 lines in two
  days. 57 sites in dax.c use error codes as control flow to break the fault/semaphore/gate/seal
  cycle.
- **The benefit is reads:** density comes from base pages (22 MB against 469 MB of guest page
  cache per VM, TASK-112; first read of /usr 0.69 s against 2.65 s without DAX). Written data is
  private to its branch: DAX shares none of it.

Options:
- **(a) Read-only DAX.** DAX maps sealed extents only; a file switches to the guest page cache at
  its first write-open (per inode, as ext4's DAX switch). Writes take the no-DAX path, 1.7x
  faster today. Removes the gate, seals' write-protects, moves on store, restaging, the open-page
  budget and ENOMEM (150.20, 153.15), refused stores, most dax.c patches and specs. Costs the
  per-inode switch (Linux allows it only at eviction), written pages in guest RAM.
- **(b) Host copy-on-write.** Clean pages map sealed extents read-only; a first store gets a
  private host page in the existing userfaultfd write-protect handler; fsync is DAX writeback,
  Fxfs allocating extents then (delayed allocation) and the host copying pages (a WRITE from the
  window). Keeps shared writable mmap on DAX. Removes the same guest machinery, about half of
  dax.c. Costs two protocol flags, host CoW, delalloc reservations.
- **(c) Keep it,** with 0.55x and the bug rate.

## 2. Two storage engines holding the same branches

extent_store (13.2k lines: lineage, records, layers, ledger, index, cleaning, rebuild) and
extent_remote (12.3k: records, forks, checkpoints, recovery, collector) each hold every branch,
and crates/fxdax/src/remote.rs (4,215 lines) keeps them agreeing; since TASK-45 every store has a
bucket. At least 13 bugs at that seam (79.4, 79.5, 79.20, 79.39, 79.71, 79.72, 79.77, 79.78,
79.82, 79.85, 79.95, 150.1, 150.6); three footer replays (160.15); two cleaners; TASK-79.82
"fixed" three times and a variant on 11-03.

- **Local disk as the bucket's write-ahead log and cache:** only unacknowledged segments and a
  part cache locally; forks, points, records, checkpoints and cleaning only in the bucket's
  protocol; one replay. Removes about 7-8k lines and the seam's bug class. Weeks.
- **A serial commit log over parallel data** (SlateDB's manifest): removes voids, links, gap
  fills, the lease GET per acknowledgement. Format change.
- **TLA+ of doc-7 once smaller:** the random scheduler can't reach 79.18 or 79.3; followers,
  specified first, have had no bugs since 10-24.

## 3. Window state with three owners, and a userspace page cache built for Zircon

extentd keeps two maps of a window (Dax, WindowState) and the VMM a third (uffd::Pages); every
change is an acknowledged stage operation with quarantine, a watchdog and SIGKILL behind it.
physmem and the userfaultfd second stage were lifted from the Zircon design (TASK-96.1) rather
than rethought. Bugs: VM-fatal EFAULTs (99, 101, 108, 119.21), quarantine against relief (106,
132, 161), one wedged VMM blocking every tenant (79.9), the watchdog killing healthy VMMs (148).

- **Split ownership:** extentd owns ranges and leases, the VMM owns pages, resolving resident
  faults itself from a page directory extentd publishes in a memfd.
- **Or the host kernel's page cache** (once DAX maps only sealed extents, 1a): uncompressed local
  cache files mapped into the window, virtio-fs DAX's way. Removes physmem, relief, quarantine and
  the uffd stage (about 6-10k lines). Costs per-tenant caps falling to memcg.

## 4. extentd is a single point of failure that can't restart

If extentd dies, every VM on the host exits (158) and up to 50 ms of acknowledged fsyncs are lost
per VM; panics poison stores (104, 105, 79.80); upgrades drain the host. Tenant limits count arena
pages only: per-connection buffers (up to about 1 TiB a tenant by the constants) and public bases
are charged to no one. Change: crash-only extentd (panic=abort, arena memfds in systemd's fd
store, VMMs pausing and reconnecting); count connection memory per tenant; an open-page quota per
VM.

## 5. Fxfs still pretends the device is a block device

The journal is a file over write-once extents (each 4 KiB journal block an extent; journal.rs
+1,797/-261; TASK-139, 149.23, 153.13, 160.25); data checksums run in "barriers off" mode though
doc-4 says barriers on (fxfs-linux fs.rs:155), the source of TASK-160.22's class; the fork is about
26% of upstream (+14,010/-3,897 of 68,409 lines), its pin never moved since 10-10. Keep Fxfs (its
LSM and objects fit extents; a new filesystem would have the same DAX problem); give journal
blocks position-derived IDs; turn barriers on and drop data checksums once 1 is decided; declare
allocator/journal/super_block fxdax-owned; merge upstream once to price drift.

## 6. Scope

Followers, shared git volumes, multi-host points and TASK-96's Durable-Objects store each add
states DAX and the store must stay correct under; none was asked for by the sandbox team. Freeze
them until 1-3 are decided.

## The radical alternative

Stock ext4 or EROFS over a copy-on-write block store carried by extent_remote's protocol: a
writable upper on virtio-blk, a read-only base on virtio-pmem DAX (measured 2.05 s first read,
1,125-1,197 tps). Removes the Fxfs fork, the guest std, fs/fxfs, the kernel patches and guest/sim,
about 180k of 290k lines; loses guest-minted extent IDs, followers and Fxfs-native shared volumes.
Worth a 1-2 week spike judged by the same gate, not a decision on paper.

## What is sound

Write-once sealed extents as the unit of sharing, caching and upload; guest-minted IDs; the
object-storage basics (create-only objects, conditional-write leases, fencing tokens, immutable
points, one deleter, 350x fewer requests than JuiceFS); one extentd per host with tenant arenas
and the VM process untrusted; read-side DAX with MAP_AHEAD; commit-after-bytes; one trust
boundary for layer records; and the engineering: guest/sim, the model test, Shuttle, TLA+ with
mutants, dead-ends, the perf gate.

## Decisions needed

1. The DAX write path: 1a, 1b, or a prototype of each judged by the perf gate.
2. Collapse the local lineage into the bucket's WAL and cache (2).
3. A spike of the radical alternative.
4. Which of followers, shared volumes, multi-host points and TASK-96 has a customer now.
5. The density target (VMs per host, bare metal or nested) and the share of a sandbox's memory
   that is base pages: the density gate measures throughput, not memory.

## Decided (the user, 2026-11-04)

1. **Sealed extents only on DAX, written pages kept apart** (TASK-164). The user first chose 1a,
   then refined it: a Redis-like 400 GB file mapped read-write with 5% written must keep the
   unwritten 95% on DAX (blocks written in earlier forks included) and spend memory only on the
   written pages. The switch is therefore per page, not per file: 1b (host copy-on-write with
   writeback allocation) or a guest-side per-page mix; the design doc decides. TASK-160.1, 160.3 and 160.6 are superseded; 160.2, 160.4 and 160.5 wait on it.
2. **Local disk becomes the bucket's WAL and cache**, after the DAX work. TASK-165.
3. **No radical spike:** improve fxdax as it is.
4. **Scope:** multi-host points and shared git volumes continue; live followers and TASK-96's
   store layer are frozen (TASK-42.11, 92.30, 92.31, 96 at low priority).
5. Density target: not yet asked.
