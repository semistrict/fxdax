---
id: doc-12
title: The fsync durability contract
type: other
created_date: '2026-10-31 22:17'
updated_date: '2026-11-06 06:43'
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
| Durable locally | the data synced, then the record written and synced (`Store::make_durable`) | within 50 ms |
| Uploaded | in object storage, a contiguous prefix, the lease still the writer's | the lag (below) |

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
| extentd killed | up to 50 ms may go, its records unwritten; a prefix stays. The VM exits 158 | kept | fxfs-guest-crash-dax-daemon-killed (1 of 296 files told of went, GCE r342), fxfs-guest-daemon-lost |
| Host reboot or kernel crash, disk kept | up to 50 ms may go; a prefix stays | kept | **nothing: no test loses a host's page cache (TASK-67)** |
| Host lost, disk with it, with a remote | the unuploaded lag goes; a prefix stays | kept | the simulation (doc-7, `crashes_anywhere`); **no guest test (TASK-67)** |
| Host lost, without a remote | everything goes | everything goes | not a recovery case: a store with no remote, or whose bucket is a file bucket beside it (an image's, `<image>.bucket`), lives on one disk; extentd refuses to start without a remote (`FXDAX_EXTENTD_REMOTE`, TASK-45) |

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

## How a host tells them apart

A VM whose daemon goes exits with status 158 (`ExtentdLost`), within the bound its test holds it
to. The README lists the rest ("How a VM process exits").

## Not promised

- That an fsync survives extentd or the host going within 50 ms of returning. An application that
  needs that must `syncfs`, which costs a round trip to the disk, and to object storage with a
  remote.
- Anything of what the lag holds, across the loss of a host.
