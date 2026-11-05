---
id: doc-11
title: Upgrading fxdax in production
type: other
created_date: '2026-10-31 20:30'
updated_date: '2026-11-05 14:50'
---
# Upgrading fxdax in production (TASK-141)

## Where things stand

| Layer | How it is versioned | What a mismatch does today |
|---|---|---|
| Fxfs format (guest kernel, `crates/fxfs`) | `LATEST_VERSION` 60, `EARLIEST_SUPPORTED_VERSION` 60 | Any bump strands every store: nothing reads an older one |
| Store files (`extent_store`) | `FORMAT`, "fxdax extent store 6" (6 names the bucket, TASK-165.6), matched exactly | Refused, opened or deleted, by its format (`extent_store::Format`, `ENOTSUP`) |
| Branches in a bucket (`extent_remote`) | `layout::FORMAT`, first in each branch's record; each framed object's trailer ends with its frame's (2026-11-03, TASK-141.2) | A daemon refuses a branch of a newer format, naming it, reading nothing else of it; the collector's pass stops, deleting nothing |
| VM process and daemon (`extentd/wire.rs`) | No handshake. New requests are new kinds, refused with `EINVAL` by an older daemon | Works if each new request is optional; nothing checks that |
| Guest and device (virtio) | Feature bits (doc-4) | Negotiated; sound |
| VM snapshots (Firecracker) | `SNAPSHOT_VERSION`: same major, minor no newer | Refused on restore |
| Daemon restart | None | Every VM on the host exits 158 (`ExtentdLost`) |

Nothing here has run in production, so the first real release sets the baseline: stores made before it are not carried forward. From then on, these rules hold.

## Rules

**1. Two releases to change a format.** Release N learns to read the new format and keeps writing the old. Release N+1 writes the new. That leaves rollback from N+1 to N possible, and rollback from N to N-1 always possible. It applies to Fxfs, store files and bucket layouts alike.

**2. Every release reads the format of the release before it.** `EARLIEST_SUPPORTED_VERSION` and the store's accepted formats span at least two releases, and a release with stores to migrate migrates them at open.

**3. Every persistent format says its version, and a reader refuses a newer one.** That includes the bucket's branches, whose records say it since 2026-11-03 (TASK-141.2).

**4. Components upgrade in one order: daemon, then VMM, then guest kernel.** Each must work with the release before it on the side that hasn't upgraded yet. A daemon of N serves a VMM of N-1, and a VMM of N serves a guest of N-1. A request a new VMM adds stays optional, as `BUILD` is: an `EINVAL` from an older daemon is not an error. Rollback runs in the reverse order.

**5. A VM never has to restart for a daemon upgrade to be safe, but in the first release it does restart.** See below.

## Upgrading a host

The first release has no handover of a running daemon's state, which holds physical memory, open stores and connections. So a daemon upgrade is a host drain:

1. Mark the host unschedulable. New VMs go elsewhere.
2. Move each VM off, by snapshot and restore through the bucket, as `scripts/migrate-gce.sh` does (TASK-28.11), or stop it if its tenant accepts that.
3. Followers go with or after their writer.
4. SIGTERM the daemon, whose health service stops reporting it as serving. Wait until its stores are closed, which makes them durable.
5. Install the daemon and VMM, start the daemon, check `/metrics` shows the new build (TASK-111.7) and uncordon.

A guest kernel upgrades as its VM next boots: it isn't part of a host drain.

An in-place handover (the old daemon passing its memory objects and connections to the new one over a socket) would turn the drain into a pause. It is deliberately out of scope here: it is a large feature whose value depends on how often the daemon must change. Revisit it once drains have run in production.

## Rollback

Rolling back is the same drain in reverse order, and needs no data migration as long as rule 1 held. A release's notes say which release it can roll back to.

A rollback past a format change is not supported: the store would be refused, by rule 3, rather than misread.

## What makes this true: tests

- **Golden stores.** Each release checks in a small store made by that release, with its Fxfs image, and CI opens the previous release's: it mounts, passes fsck and reads back what it was made with.
- **A skew matrix.** Nightly, on GCE, the guest suites run with one component at the previous release's tag: daemon N with VMM N-1, and VMM N with guest kernel N-1.
- **A drain rehearsal.** Nightly, `scripts/migrate-gce.sh` moves a VM from a host of N-1 to one of N, and back.
