---
id: TASK-136.6
title: 'Attach a repo on path lookup, through the sandbox manager'
status: In Progress
assignee: []
created_date: '2026-10-30 09:08'
updated_date: '2026-11-08 05:35'
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
- [ ] #1 A lookup under a configured path attaches and mounts the repo, and the lookup returns once it is mounted
- [ ] #2 A repo the sandbox manager refuses fails the lookup with an error, and leaves nothing mounted
- [ ] #3 Concurrent lookups of one repo attach it once
- [ ] #4 An idle repo's mount expires, closing its store and keeping its writable layers, and its next lookup mounts it again
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fxfs: ObjectManager::close_volume, open_volume's inverse (lazy volumes): flush what the journal holds of the volume, then forget its store once nothing else holds it; Unavailable if in use, ReadOnlyFilesystem if it can't flush. Tests in volume/lazy.rs (closed and open again, a crash after, in use, read-only). fxfs-linux's unmount calls it; /proc/fs/fxfs/TAG says volumes_open.
2. crates/fxfs-automount: an autofs (protocol 5, indirect) daemon, fxfs-automount ROOT FILESYSTEM: a lookup's name mounted from the volume of that name, imported first from the manifest the sandbox manager answers with; refused, the lookup fails ENOENT, nothing left; expiry by autofs's timeout unmounts (closing the volume). Upcall over vsock (port 5054), a line protocol (upcall.rs). The daemon's decisions unit-tested over fakes on any OS; the autofs and Fxfs glue Linux's.
3. Guest test fxfs-guest-automount(-dax): publisher publishes a volume; consumer runs the automounter on /repos with a host stand-in for the sandbox manager (shared_volumes::host::Manager): lookup mounts, 8 concurrent lookups attach once, refusal leaves nothing, expiry closes volumes (volumes_open) and the next lookup remounts unasked; reboot remounts unasked. Host checks the asks.
4. doc-10's Automount section; OPEN_QUESTIONS decisions (autofs over a d_automount of our own; every unmount closes its volume).
<!-- SECTION:PLAN:END -->
