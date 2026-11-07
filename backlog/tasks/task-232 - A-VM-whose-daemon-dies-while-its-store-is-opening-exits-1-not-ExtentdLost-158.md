---
id: TASK-232
title: >-
  A VM whose daemon dies while its store is opening exits 1, not ExtentdLost
  (158)
status: To Do
assignee: []
created_date: '2026-11-07 13:25'
labels:
  - bug
dependencies: []
priority: high
ordinal: 666000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67) on GCS, GCE r831 round 29 (run FXDAX_SEED=13088868158266494024, round seed 13161760618986386206; plan: 1000 ms added to every bucket request, slot 0's pgbench VM killed at 78 s and booted again, the daemon killed at 89 s). The VM booted again was opening its store (its recovery from GCS, every request a second late, took more than the 11 s left) when the daemon died; its OPEN failed EIO, device creation failed, and Firecracker exited 1: `Cannot create extent-store device: Cannot open the extent store .../slot-0: ... EIO (errno 5)`. doc-12 and the README promise a VM whose daemon goes exits 158 (ExtentdLost), which is how a VM manager tells the daemon's loss from a broken store or VM. The VMM registers what exits 158 on the daemon's loss only once the store opened (vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:185-205 opens, stop_with_daemon :555-561 after); a loss during the open is the open's error. Since TASK-231 the client marks the daemon lost before its requests fail (crates/fxdax/src/extentd/client.rs), so the open can say it failed for that (a typed error, or RemoteExtentStore::daemon_lost on the failed connection), and the VMM exit 158 then.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store whose daemon goes during its opening fails with an error that says so, shown by a test that fails first
- [ ] #2 Firecracker exits 158 when its daemon goes as it creates its extent device, as after (a VMM test, or a guest case killing the daemon as its VM boots)
- [ ] #3 fxfs-guest-soak accepts no exit but 158 or a kill for a VM booted before the daemon died
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
