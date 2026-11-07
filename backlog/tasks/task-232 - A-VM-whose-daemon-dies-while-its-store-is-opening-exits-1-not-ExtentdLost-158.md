---
id: TASK-232
title: >-
  A VM whose daemon dies while its store is opening exits 1, not ExtentdLost
  (158)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 13:25'
updated_date: '2026-11-07 15:21'
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
- [x] #1 A store whose daemon goes during its opening fails with an error that says so, shown by a test that fails first
- [x] #2 Firecracker exits 158 when its daemon goes as it creates its extent device, as after (a VMM test, or a guest case killing the daemon as its VM boots)
- [x] #3 fxfs-guest-soak accepts no exit but 158 or a kill for a VM booted before the daemon died
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax client: DaemonConnection, a connection before its store opens, whose on_lost is said before OPEN; RemoteExtentStore::connect* open over one. The connection's end tells on_lost before the requests waiting fail, so a VMM exits 158 before the open's failure reaches it; an open failed by the daemon's going says 'the extent daemon went'.
2. Test first: extentd::tests::daemons_going_as_stores_open_are_told_of_first (hand daemon taking the OPEN and going; failpoint client::requests_failed holding the end).
3. vendor/firecracker (fork branch land-k5 on fxdax-v1.16.1): device.rs connects, registers exit 158, then opens (boot, restore, follower reopen).
4. Guest case fxfs-guest-daemon-lost: a third VM whose daemon (the test's own) takes its OPEN and closes; Firecracker exits 158. Red on the old VMM, green on land-k5, on GCE r833, jailed.
5. AC #3: fxfs-guest-soak's load_problems already accepts only 0/kill/158 (158 when lossy) for a VM: no change.
6. Linux Docker + macOS fxdax tests, FC extent tests, upstream-changes.py --check; push land-k5 (fxdax and fork) and run CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
land-k5 ac2e7e1d7 (fork land-k5 7d0a0970e on 99c2b05c5): CI 42379721997 green, all 11 jobs. AC#1: extentd::tests::daemons_going_as_stores_open_are_told_of_first red before (['open failed'] against ['daemon lost', 'open failed']), green after; the open fails 'opening the extent store at store: the extent daemon went: EIO (errno 5)'. AC#2: fxfs-guest-daemon-lost's third VM (its daemon the test's own, taking the OPEN and closing), jailed, on GCE r833: red on the old VMM (fork 05dd8e3df: "opening's VM exited exit status: 1 (signal None), not 158", Firecracker 'Cannot create extent-store device ... exit_code=1'), green on land-k5 (OK 1/1; 'extent device extents: the extent daemon went; stopping the VM'). AC#3: fxfs-guest-soak's load_problems already accepts only 0, a kill, or 158 when lossy (r831 round 29 failed on the 1): unchanged. fxdax 315/315 on macOS before the rebase; Linux Docker extentd/uffd (163) and VMM extent tests (23) after it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A VM whose daemon dies as its store opens stops ExtentdLost (158), not 1: DaemonConnection has what the daemon's going does said before OPEN, and the connection's end tells of it before the requests waiting fail; an open failed so says the daemon went. The VMM (fork 7d0a0970e) connects, registers exit 158, then opens. Verified: daemons_going_as_stores_open_are_told_of_first red before, green after; fxfs-guest-daemon-lost's new third VM, jailed on GCE r833, exited 1 on the old VMM and 158 on land-k5; land-k5 CI 42379721997 and land-hk5 CI 42382617461 green (workspace on Linux), fxdax 315/315 on macOS, scripts/upstream-changes.py --check passing. Landed on main as land-hk5 dd5916063.
<!-- SECTION:FINAL_SUMMARY:END -->
