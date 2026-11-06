---
id: doc-11
title: Upgrading fxdax in production
type: other
created_date: '2026-10-31 20:30'
updated_date: '2026-11-06 00:44'
---
# Upgrading fxdax in production (TASK-141)

AGENTS.md's rule, the user's (2026-11-05): **no migrations, ever**. A format changes outright whenever that is the better design, and new code refuses the old one (`Error::Format` and its kind): no release reads, converts or upgrades another's data, no component is kept working beside one of another format, and nothing is kept to read old data. This replaces the two-release plan (formats read one release back, migrated at open, a skew matrix, golden stores, rollback) this document held before.

## Where things stand

| Layer | How it is versioned | What a mismatch does today |
|---|---|---|
| Fxfs format (guest kernel, `crates/fxfs`) | `LATEST_VERSION` 61, `EARLIEST_SUPPORTED_VERSION` 60 (`serialized_types/types.rs`) | A version-60 filesystem still mounts and is upgraded: against the rule, TASK-141.1 |
| Store files (`extent_store`) | `FORMAT`, "fxdax extent store 6" (6 names the bucket, TASK-165.6), matched exactly | Refused, opened or deleted, by its format (`extent_store::Format`, `ENOTSUP`) |
| Branches in a bucket (`extent_remote`) | `layout::FORMAT` 2, first in each branch's record (TASK-165.3); each framed object's trailer ends with its frame's (TASK-141.2) | A daemon refuses a branch of any other format, older ones too, naming it, reading nothing else of it; the collector's pass stops, deleting nothing |
| VM process and daemon (`extentd/wire.rs`) | No handshake. `BUILD` is optional: an older daemon's `EINVAL` is let be | Nothing refuses a VM process of another build: TASK-141.3 |
| Guest and device (virtio) | Feature bits (doc-4) | Negotiated: a guest runs on a device of another build: TASK-141.3 |
| VM snapshots (Firecracker) | `SNAPSHOT_VERSION`: same major, minor no newer | Refused on restore |
| Daemon restart | None | Every VM on the host exits 158 (`ExtentdLost`) |

## Rules

**1. Every persistent and wire format says its version, and a reader refuses any version but its own,** naming it, and reads or writes nothing else of what it refused.

**2. A format change is one release.** The new code writes and reads the new format alone. Stores, branches and filesystems of the old one are refused; they are made again, not converted.

**3. The components of one release run together.** A VM process and a daemon, or a guest and its device, of different wire formats refuse each other when they meet (TASK-141.3), rather than run with whatever requests both happen to know.

**4. Rolling back is the drain below, to a release that reads the same formats.** A rollback past a format change is refused by rule 1, not supported.

## Upgrading a host

There is no handover of a running daemon's state, which holds physical memory, open stores and connections. So a daemon upgrade is a host drain:

1. Mark the host unschedulable. New VMs go elsewhere.
2. Move each VM off, by snapshot and restore through the bucket, as `scripts/migrate-gce.sh` does (TASK-28.11), to a host whose release reads its formats; or stop it, if its tenant accepts that.
3. Followers go with or after their writer.
4. SIGTERM the daemon, whose health service stops reporting it as serving. Wait until its stores are closed, which makes them durable (TASK-141.4: today SIGTERM exits without closing them).
5. Install the daemon and VMM, start the daemon, check `/metrics` shows the new build (TASK-111.7) and uncordon.

A guest kernel upgrades as its VM next boots, with the VMM it then runs on: it isn't part of a host drain.

An in-place handover (the old daemon passing its memory objects and connections to the new one over a socket) would turn the drain into a pause. It is deliberately out of scope here: it is a large feature whose value depends on how often the daemon must change. Revisit it once drains have run in production.

## What makes this true: tests

- **Refusals.** Each format's reader refuses another, by a test: `host_store.rs`'s `older_stores_are_refused_by_their_format`, `record.rs`'s `records_of_other_formats_are_refused`, `collector.rs`'s `passes_stop_at_branches_of_newer_formats`; Fxfs's version 60 (TASK-141.1); the wire's and the device's (TASK-141.3).
- **A drain rehearsal.** Nightly, `scripts/migrate-gce.sh` moves a VM off a host and back, both of one build (TASK-141.4).

No golden stores of earlier releases and no skew matrix: what they would test, the rule refuses.
