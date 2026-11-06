---
id: TASK-203
title: >-
  A snapshot of a VM whose window holds private pages records their sources, not
  their bytes
status: To Do
assignee: []
created_date: '2026-11-06 06:54'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the docs pass of 2026-11-06 (doc-4 and doc-13 against the code). The guest says, freezing (fxfs_dax_frozen, guest/linux/fs/fxfs/dax.c:3476-3489), that every private page is rebound to its sealed extent before a snapshot, and that "those it can't, warned of: the device refuses the snapshot". Nothing refuses it: Firecracker's Extent::take_snapshot (vendor/firecracker 96cb97976, src/vmm/src/devices/virtio/extent/device.rs:473-497) checks the device read-only and frozen, forks the store (ExtentStore::fork_quiescent, which checks open extents, not private pages) and records window_mappings(); Window::mappings (crates/fxdax/src/dax.rs:943-957) records each mapping's source, an extent or zeros, never a private page. A snapshot taken with a page still private (a writeback that failed, or a rebind that couldn't be made, the guest only warning) restores that page as its source's old bytes, under a guest whose page cache state says otherwise. doc-4 (Snapshots) and doc-13 now say what the code does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A snapshot of a store whose window holds a private page fails, naming the device and the count, and the VM resumes unchanged; or the private page's bytes are restored with it
- [ ] #2 A test takes a snapshot with a private page left (a rebind refused) and shows the refusal or the bytes restored
- [ ] #3 dax.c's comment, doc-4 and doc-13 say the same as the code
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
