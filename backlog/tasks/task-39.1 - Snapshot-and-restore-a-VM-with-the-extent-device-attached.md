---
id: TASK-39.1
title: Snapshot and restore a VM with the extent device attached
status: In Progress
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-17 22:07'
labels: []
dependencies: []
parent_task_id: TASK-39
priority: high
type: feature
ordinal: 111000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Firecracker refuses to snapshot a VM with the extent device (vendor/firecracker/src/vmm/src/device_manager/persist.rs: 'extent-store devices do not support snapshotting'), so zygote restore, suspend-to-RAM, live migration and memory snapshots all break. A snapshot must capture the device's state (queues, in-flight requests, the DAX window's mappings of extentd memory) and pin the store at the point the guest's memory reflects; a restore must reattach to that store, or a fork of it, and remap the window.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Firecracker snapshots and restores a VM with the extent device, DAX window included, and the restored guest reads and writes its filesystem, shown by a guest test
- [ ] #2 A snapshot names the store point it needs (a fork made at the snapshot), so restoring twice from one snapshot gives two independent VMs (zygote), shown by a test
- [ ] #3 Suspend-to-RAM and live migration to another host (store taken over through object storage) work, shown by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Decision (overnight, user asleep): the snapshot point is a frozen filesystem. Extent IDs carry their branch, and a restored guest keeps its branch in RAM, so a restore into a new branch must happen at a point where nothing is open, and the guest must learn its new branch before it mints again. Freezing (FIFREEZE) gives that: Fxfs syncs durably and holds its background work; thawing re-reads INFO.
1. Guest: freeze_fs pauses Fxfs's compactions, syncs, and closes the device to mutating requests (WRITE, FREE, COMMIT, writable MAP) until thaw; reads and read-only MAPs go on. It tells the device it is frozen (a FREEZE request, new in doc-4). unfreeze_fs reopens it, re-reads INFO (branch, sequences, capacity) and resumes compactions.
2. extentd: the window's mappings are listed (extent, offset, len, window offset, writable): a new MAPPINGS op. OPEN can name a snapshot point to fork the new store from, if the store does not exist yet (OPEN_FORK_FROM), serialized on the point.
3. Firecracker extent device: Persist (ExtentState: config, config space, virtio state, the window's guest range, its mappings, the snapshot's store point). Snapshotting needs a frozen guest. prepare_save drains the worker and takes its queue back. The store is forked at the snapshot into a point beside the snapshot (a path the snapshot API names, or <snapshot>.extent-<id>). Restore takes overrides of the store's path; it opens the new store forked from the point, recreates the window at the same guest range (from_state), and replays the mappings. The refusals in lib.rs and persist.rs go.
4. Tests: a guest test in API mode: boot, write files (DAX and not), freeze, snapshot, restore twice into two stores (zygote), each thaws, writes its own file, remounts, checks; the host checks both stores. Suspend: snapshot, kill, restore, continue. Migration: two daemons sharing a bucket (file:// object store), restore on the second.
5. Docs: doc-4 (FREEZE, MAPPINGS, OPEN_FORK_FROM), README (snapshot contract: freeze before, thaw after).
<!-- SECTION:PLAN:END -->
