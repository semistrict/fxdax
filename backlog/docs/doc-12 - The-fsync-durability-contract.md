---
id: doc-12
title: The fsync durability contract
type: other
created_date: '2026-10-31 22:17'
updated_date: '2026-11-10 12:24'
---
# What an acknowledged fsync survives (TASK-66)

doc-2's requirement 2 is the rule: a crash may lose a suffix of recent history, and the filesystem
recovers to a prefix of it. fsync keeps its ordering, and need not wait for durability. This says
what that means, failure by failure. The *Tested by* column names what shows it, or says nothing
does: a claim no test shows is a design statement, not a result.

*Revised 2026-11-04 (doc-13, TASK-164):* with DAX as without it, writeback is the write path. A DAX
file's written pages are private pages in host memory, as the page cache's are folios in guest RAM,
and reach the store only as writeback `WRITE`s them, then commits. The window plays no part in
recovery.

## The stages a commit goes through

| Stage | What it means | When |
|---|---|---|
| Ordered | `COMMIT` returned: extentd has it, in order: its data in the store's files, its commit record in extentd's memory (`Pending`, `Store::commit_locked`) | as `fsync` returns |
| Durable locally | the data synced, then the record written and synced (`Store::make_durable`) | within 50 ms at the median (32 ms on main); the crash bound, 1 s, at most (below) |
| Uploaded | in object storage, a contiguous prefix, the lease still the writer's | the lag (below) |

*Revised 2026-11-08 (TASK-221, the user's decision: the window is what main achieves):* "within
50 ms" is the **median**: extentd makes the commits ordered durable every 50 ms, a flush behind the
host's disk at worst. Measured on main (08d444ef7, GCE n2-standard-16, jailed, 3 rounds each of
fxfs-guest-pgbench and -pgbench-dax, ~365,000 commits a round, extentd's `durable_lag_*` stats): the
median 32 ms or less, p99 64 ms or less, 85% within 50 ms, and the longest under 1,024 ms (about
0.7% of commits over 64 ms). What a daemon or host crash may lose is bounded by that longest lag:
**the crash bound, 1 s**. The daemon-killed test holds extentd to it (`CRASH_LOSS_BOUND_MS`), and the
pgbench cases to the median (`durable_lag_within_50ms`). Shortening the tail is performance work
(TASK-221's notes: commits whose sealing copies whole extents into the open segment).

A guest's **fsync, fdatasync, msync and O_SYNC** wait for *ordered*. **sync, syncfs, unmount and
freeze** ask for `COMMIT_DURABLE`, and wait for *durable locally*, and with a remote, for *uploaded*.
So an acknowledged fsync is not yet durable: it is ordered, and survives what leaves extentd alive.

The lag is bounded by commits and bytes (`remote::Options`: 4096 commits, 1 GiB, by default) and in
time by the segment's age. Past the bound, commits wait: the lag is what a lost host can cost, and
it stays within the bound.

## What each failure keeps

| Failure | fsync acknowledged | sync acknowledged | Tested by |
|---|---|---|---|
| Guest crash or panic, VM reset | kept | kept | fxfs-guest-crash, -crash-dax; guest/sim's `cow_crash_between_bytes_and_commit` (a commit held after its bytes) |
| VMM killed (SIGKILL) | kept | kept | fxfs-guest-crash-dax-killed |
| extentd killed | what was ordered within the crash bound (1 s) may go, its records unwritten; a prefix stays, a file whose fsync returned in that time found short or empty at worst. The VM exits 158 | kept | fxfs-guest-crash-dax-daemon-killed (1 of 296 files told of went, GCE r342), fxfs-guest-daemon-lost |
| Host reboot or kernel crash, disk kept | what was ordered within the crash bound may go; a prefix stays | kept | **nothing: no test loses a host's page cache (TASK-67)** |
| Host lost, disk with it, with a remote | the unuploaded lag goes; a prefix stays | kept | the simulation (doc-7, `crashes_anywhere`); **no guest test (TASK-67)** |
| Host lost, without a remote | everything goes | everything goes | not a recovery case: a store with no remote, or whose bucket is a file bucket beside it (an image's, `<image>.bucket`), lives on one disk; extentd refuses to start without a remote (`FXDAX_EXTENTD_REMOTE`, TASK-45) |
| The bucket failing for good: out of reach, full, refusing its credentials | kept: waits at the lag bound, then fails nothing (below) | kept: waits | extent_remote's `writes_failing_say_why_until_they_land` (a full disk's `ENOSPC`, logged); fxdax's `commits_wait_at_the_lag_bound_while_the_bucket_is_full`, `commits_wait_at_the_lag_bound` (TASK-314) |

Every row keeps ordering: what survives is a prefix of the commits, never a later commit without an
earlier one. Filesystems recover by replaying their journal to that prefix. Open-but-unnamed files
are freed as the store is next mounted.

## What a guest sees meanwhile

- **A store through a mapping, or a `write(2)`,** is in the page, not in a commit: a private page of
  the VM's on DAX, a folio of the guest's page cache without. It is covered by the next `msync` or
  `fsync` of the file, and may reach the store before that only by writeback (`sync`, the flusher
  at ~30 s, the guest keeping its private pages under `private_max`), each page as writeback copied
  it. Every crash, of the guest, the VMM or extentd, loses what wasn't written back: private pages
  go with the device.
- **A `write(2)` cut short** leaves the bytes it copied, as on ext4: nothing it writes moves.
- **The device's store failing** fails the store: a commit that failed part way leaves it refusing
  what follows (`State::failing`), and the guest's writes and fsyncs return errors.
- **The bucket failing**, for a while or for good, fails nothing (TASK-314; doc-7, decision 3: as
  a hard NFS mount hangs). Every request it fails is retried, from 50 ms apart, doubling, to 5 s,
  until it is answered. Meanwhile the guest's `sync`, `syncfs`, unmount and freeze wait at once,
  as they wait for *uploaded*; its fsyncs go on, ordered and made durable locally, until the lag
  reaches its bound (4,096 commits or 1 GiB of write-ahead log), then wait too; its writes go on,
  and so do its reads of what the host has, but a page-in of what only the bucket holds waits. Its
  kernel says so as a hang does (`fxfs: Write I/O request blocked for N s`, hung tasks), and
  nothing returns an error. Once the bucket answers, within 5 s of its next retry, all of it goes
  on, and nothing was lost: what waited was durable locally all along. *Decided 2026-11-10
  (TASK-314), the most conservative of the options:* failing commits after a bound instead would
  give the guest errors for data that is safe on the host's disk, and a guest filesystem that
  errors on fsync shuts down, or retries what Linux has already dropped from its page cache: a
  failure that frees the bucket's disk, or renews its credentials, cures, made one that loses
  data. It is loud instead: extentd logs each kind of request failing as the run of failures
  starts, a warning naming the bucket's own error (a file bucket's `No space left on device`,
  S3's `AccessDenied`), then every 30 s with how many failed, and as it answers again
  (`extent_remote::store`, `FAILING_EVERY`); counts them by kind (the stats line's `failed-NAME`,
  GetStats' `ObjectStorageStats.failures`, the metric `fxdax_extentd_object_storage_failures_total`),
  beside how long each store's oldest commit has waited for its upload
  (`fxdax_extentd_upload_oldest_pending_seconds`), what an operator alerts on; and warns as a
  commit first waits at the bound. *Added 2026-11-10 (TASK-317):* meanwhile extentd's memory does
  not grow with what waits, which is on the host's disk: what it holds of it is what it uploads at
  once, its upload memory (`FXDAX_EXTENTD_UPLOAD_MEMORY`, 512 MiB), and it gives back what it
  frees (doc-7, Uploads, Memory). Before, a backlog held its uploads twice and the allocator kept
  what it freed, 4-6 GB of anonymous memory, and the host's OOM killer took the daemon, every VM
  it served with it.

## How a host tells them apart

A VM whose daemon goes exits with status 158 (`ExtentdLost`), within the bound its test holds it
to. The README lists the rest ("How a VM process exits").

## Not promised

- That an fsync survives extentd or the host going within the crash bound (1 s) of returning. An application that
  needs that must `syncfs`, which costs a round trip to the disk, and to object storage with a
  remote.
- Anything of what the lag holds, across the loss of a host.
