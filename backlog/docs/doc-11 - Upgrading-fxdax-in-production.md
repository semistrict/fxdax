---
id: doc-11
title: Upgrading fxdax in production
type: other
created_date: '2026-10-31 20:30'
updated_date: '2026-11-11 12:49'
---
# Upgrading fxdax in production (TASK-141)

AGENTS.md's rule, the user's (2026-11-05): **no migrations, ever**. A format changes outright whenever that is the better design, and new code refuses the old one (`Error::Format` and its kind): no release reads, converts or upgrades another's data, no component is kept working beside one of another format, and nothing is kept to read old data. This replaces the two-release plan (formats read one release back, migrated at open, a skew matrix, golden stores, rollback) this document held before.

## Where things stand

| Layer | How it is versioned | What a mismatch does today |
|---|---|---|
| Fxfs format (guest kernel, `crates/fxfs`) | `LATEST_VERSION` 61; `EARLIEST_SUPPORTED_VERSION` is `LATEST_VERSION` (`serialized_types/types.rs`) | A filesystem of any other version is refused at mount, its version named (`FxfsError::InvalidVersion`, `EINVAL` in the guest), writing nothing; so are a manifest, and a base's layers, of another (TASK-141.1) |
| Store files (`extent_store`) | `FORMAT`, "fxdax extent store 10" (6 named the bucket, TASK-165.6; 7 places bucket data by its location there and holds one branch, no branch records, TASK-165.7, TASK-165.8; 8 has no index, TASK-165.9; 9 a commit spanning segments, TASK-92.41; 10 logs whose headers name their segments, TASK-341), matched exactly | Refused, opened or deleted, by its format (`extent_store::Format`, `ENOTSUP`) |
| Branches in a bucket (`extent_remote`) | `layout::FORMAT` 3 (a commit spanning segments, TASK-92.41), first in each branch's record (TASK-165.3); each framed object's trailer ends with its frame's (TASK-141.2) | A daemon refuses a branch of any other format, older ones too, naming it, reading nothing else of it; the collector's pass stops, deleting nothing |
| VM process and daemon (`extentd/wire.rs`) | `wire::VERSION` 1, in `OPEN`'s `args[4]` and its reply's (TASK-141.3); `the_wire_is_its_version` fails on a change to the wire without a new one | The daemon refuses a VM process of another (`EPROTONOSUPPORT`, logging both), and the VM process a daemon of another, naming both; every request is required: a refusal of `BUILD` or `TRANSFER` fails the connection |
| Guest and device (virtio) | `VIRTIO_EXTENT_VERSION` 1, the configuration's first field (doc-4, TASK-141.3); a snapshot's device's in its state | The driver refuses a device of another at probe, naming both; the device refuses a driver that didn't take every feature it offers (features say its configuration, no negotiation); a VMM refuses a snapshot whose device was of another |
| VM snapshots (Firecracker) | `SNAPSHOT_VERSION`: same major, minor no newer | Refused on restore |
| Daemon restart | None | Every VM on the host exits 158 (`ExtentdLost`); SIGTERM drains first, every store closed, durable and uploaded (TASK-141.4) |

## Rules

**1. Every persistent and wire format says its version, and a reader refuses any version but its own,** naming it, and reads or writes nothing else of what it refused.

**2. A format change is one release.** The new code writes and reads the new format alone. Stores, branches and filesystems of the old one are refused; they are made again, not converted.

**3. The components of one release run together.** A VM process and a daemon, or a guest and its device, of different wire formats refuse each other when they meet (TASK-141.3), rather than run with whatever requests both happen to know.

**4. Rolling back is the drain below, to a release that reads the same formats.** A rollback past a format change is refused by rule 1, not supported.

## Upgrading a host

The runbook is `docs/upgrading-a-host.md`: these steps, how each shows, and what each format change refuses.

There is no handover of a running daemon's state, which holds physical memory, open stores and connections. So a daemon upgrade is a host drain:

1. Mark the host unschedulable. New VMs go elsewhere.
2. Move each VM off, by snapshot and restore through the bucket, as `scripts/migrate-gce.sh` does (TASK-28.11), to a host whose release reads its formats; or stop it, if its tenant accepts that.
3. Followers go with or after their writer.
4. SIGTERM the daemon, which drains (`extentd::drain`, TASK-141.4): its health service reports NOT_SERVING, it takes no more connections nor control calls, cuts off what VMs are left, closes every store, durable, and waits for their uploads, within `FXDAX_EXTENTD_DRAIN` seconds (60): exit 0 drained, 1 past the bound.
5. Install the daemon and VMM, start the daemon, check `/metrics` shows the new build (TASK-111.7) and uncordon.

A guest kernel upgrades as its VM next boots, with the VMM it then runs on: it isn't part of a host drain.

An in-place handover (the old daemon passing its memory objects and connections to the new one over a socket) would turn the drain into a pause. It is deliberately out of scope here: it is a large feature whose value depends on how often the daemon must change. Revisit it once drains have run in production.

## What makes this true: tests

- **Refusals.** Each format's reader refuses another, by a test: `host_store.rs`'s `older_stores_are_refused_by_their_format`, `record.rs`'s `records_of_other_formats_are_refused`, `collector.rs`'s `passes_stop_at_branches_of_newer_formats`; `super_block.rs`'s `test_mount_of_the_previous_version_is_refused` and `base.rs`'s `test_base_layers_of_older_versions_refused` and `test_base_manifests_refused` for Fxfs's (TASK-141.1); the wire's, extentd's `vm_processes_of_other_wire_versions_are_refused` and `daemons_of_other_wire_versions_are_refused`, with codec.rs's `the_wire_is_its_version`; the device's, the guest case `extent-guest-other-version` and Firecracker's `drivers_take_every_feature_offered` and `snapshots_of_other_protocols_are_refused` (TASK-141.3).
- **The drain.** `crates/fxdax/tests/drain.rs`'s `sigterm_closes_every_store_durable_and_uploaded`: a VM's ordered commit, SIGTERM, and a later placement's daemon restores the branch from the bucket with it; extentd's `drains_close_every_connection_and_take_no_more` and `drains_give_up_past_their_bound` (TASK-141.4).
- **A drain rehearsal.** Nightly, `scripts/migrate-gce.sh` moves a VM off a host and back, both of one build (TASK-141.4).

No golden stores of earlier releases and no skew matrix: what they would test, the rule refuses.
