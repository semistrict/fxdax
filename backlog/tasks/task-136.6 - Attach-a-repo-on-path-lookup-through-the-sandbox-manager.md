---
id: TASK-136.6
title: 'Attach a repo on path lookup, through the sandbox manager'
status: Done
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-11-08 11:22'
labels: []
dependencies:
  - TASK-136.2
  - TASK-136.4
  - TASK-136.5
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 473000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A path lookup attaching a repo combines with gyit's path-is-the-URL idea. Since only the sandbox manager attaches, the lookup has to wait on an upcall through a guest agent to it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A lookup under a configured path attaches and mounts the repo, and the lookup returns once it is mounted
- [x] #2 A repo the sandbox manager refuses fails the lookup with an error, and leaves nothing mounted
- [x] #3 Concurrent lookups of one repo attach it once
- [x] #4 An idle repo's mount expires, closing its store and keeping its writable layers, and its next lookup mounts it again
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fxfs: ObjectManager::close_volume, open_volume's inverse (lazy volumes): flush what the journal holds of the volume, then forget its store once nothing else holds it; Unavailable if in use, ReadOnlyFilesystem if it can't flush. Tests in volume/lazy.rs (closed and open again, a crash after, in use, read-only). fxfs-linux's unmount calls it; /proc/fs/fxfs/TAG says volumes_open.
2. crates/fxfs-automount: an autofs (protocol 5, indirect) daemon, fxfs-automount ROOT FILESYSTEM: a lookup's name mounted from the volume of that name, imported first from the manifest the sandbox manager answers with; refused, the lookup fails ENOENT, nothing left; expiry by autofs's timeout unmounts (closing the volume). Upcall over vsock (port 5054), a line protocol (upcall.rs). The daemon's decisions unit-tested over fakes on any OS; the autofs and Fxfs glue Linux's.
3. Guest test fxfs-guest-automount(-dax): publisher publishes a volume; consumer runs the automounter on /repos with a host stand-in for the sandbox manager (shared_volumes::host::Manager): lookup mounts, 8 concurrent lookups attach once, refusal leaves nothing, expiry closes volumes (volumes_open) and the next lookup remounts unasked; reboot remounts unasked. Host checks the asks.
4. doc-10's Automount section; OPEN_QUESTIONS decisions (autofs over a d_automount of our own; every unmount closes its volume).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: implemented on land-m2 (068c68459 close_volume, 33b4a3be2 fxfs-linux unmount closes, 584baf4b9 fxfs-automount, eed826db1 guest cases, 02284af7f doc-10 and OPEN_QUESTIONS), CI 42451532378. Verified so far: fxfs volume::lazy 8/8 (close_volume's three tests new, red before as the function didn't exist; found and fixed on the way: a graveyard flush on a read-only filesystem waited for ever, and replay reopens a closed volume while the journal names it, now what the test and doc say); fxfs + fxfs-posix + fxfs-automount 885/885 on macOS; fxfs-automount 13/13 (daemon decisions over fakes, upcall protocol); Linux (musl) check and clippy clean; guest test crates check for host and guest (aarch64 musl); guest Rust (fxfs-linux) builds for aarch64-unknown-none-softfloat. Not yet run: the guest cases fxfs-guest-automount(-dax), fxfs-guest-shared-volumes(-dax) and fxfs-guest-volumes(-dax) (regressions of unmount closing), which need a GCE host: the one kept host (fxdax-keep-n2-standard-4-us-west1-b-1) is running, and the rule is to reuse a stopped kept host, never a fresh one. Not landable until they pass.

2026-11-08 GCE (fxdax-test-r1021-a, us-east4-a, n2-standard-4, nested, release guest kernel, at 02284af7f; host stopped after): fxfs-guest-automount OK (7 s) and -dax OK (8 s): a lookup mounts, 8 concurrent lookups of one name attach once, a refusal fails ENOENT leaving no mount nor volume, idle mounts expire and close their volumes (volumes_open back to before), the next lookup and a reboot remount with no ask (the host's count of asks exact). Regressions of every unmount closing its volume: fxfs-guest-shared-volumes OK and -dax OK, fxfs-guest-volumes OK and -dax OK. land-m2 rebased onto main as a38426778, CI 42455316995.

Verified 2026-11-08: on main, GitHub CI 42469441083 green at 08d444ef7 (macOS and Linux workspace, shuttle, sim, guest-sim, firecracker), scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A path lookup under a configured path attaches and mounts the repo through the sandbox manager; a refusal fails ENOENT leaving nothing mounted; concurrent lookups attach once; idle mounts expire, closing their stores and keeping their writable layers, and remount on the next lookup (land-m2, a38426778, in land-all14). GCE fxdax-test-r1021-a: fxfs-guest-automount and -dax pass.
<!-- SECTION:FINAL_SUMMARY:END -->
