---
id: TASK-277
title: >-
  fxfs-guest-snapshot-dax hangs after a PCI restore: the restored guests never
  reach the host over vsock
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 05:02'
updated_date: '2026-11-09 08:07'
labels:
  - bug
  - pci
  - snapshot
dependencies: []
priority: high
ordinal: 703000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE TASK-275 boot-new run (fxdax-keep-c3-standard-8-us-east4-a-1, main 0f011561a, Firecracker fxdax-v1.16.1-r2 68d597175, release guest kernel): fxfs-guest-snapshot-dax passed round 1, then timed out in round 2 of 10 (120 s). Both restored VMs (a, b) load in ~25 ms and print `restored`, then nothing: no `thawing`. The guests idle (no D-state task, guest-agent in S, CPU 0 in safe_halt). The host test thread sits in accept4 on the vsock listener where each restored guest asks its name on a new connection (test_fxfs_guest_snapshot.rs: the restore resets the connection), so neither guest's connect reached the host. Firecracker logged only VcpuEvent::Resume with immediate_exit. The same host and case passed 10/10 on 557b49a70 (MMIO), and on PCI in TASK-267's boot-pci run (10/10). Suspect: the vsock device's queue notification or MSI-X interrupt lost across a PCI snapshot restore. Artifacts: docs/measurements/perf-suite-pci-vs-before-2026-11-08/boot-new/e2e/fxfs-guest-snapshot-dax/round-2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Root cause of the lost post-restore vsock connection found and fixed
- [x] #2 fxfs-guest-snapshot-dax passes 30 rounds in a row over PCI on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce on a stopped n2 GCE host (40 rounds, Firecracker debug logs, guest prints of the frozen connection's end and each name connection). 2. Root cause from the hung guest's stacks and the guest kernel's vsock code. 3. Fix in Firecracker's vsock (general commit, new branch off fxdax-v1.16.1-pci, series re-cut as fxdax-v1.16.1-r3), unit tests. 4. Guest case: a name connection that ends unread is made again. 5. A/B: fixed build 40 rounds on a second n2 host. 6. Quick KVM tests + PCI guest cases, snapshot case >=30 rounds, release guest round; rebase, push, CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced on fxdax-keep-n2-standard-4-us-west1-a-1 (main + guest prints, Firecracker r2 68d597175, Debug log): round 2 of 40 hung (round 1 passed). Source VM's log, the last 0.3 ms before the pause: host shuts the frozen connection (op 4), guest closes (op 3) and sends a REQUEST on a new port (op 1), the muxer puts its RESPONSE (op 2, guest port 181040319) in the RX queue; then pause and snapshot. Both restored guests print 'connect: Ok(())' and block in read forever (vsock_connectible_recvmsg); the host's listeners never accept. Root cause: TRANSPORT_RESET resets only sockets in Linux's connected table; a connecting socket enters it on taking OP_RESPONSE (virtio_transport_common.c recv_connecting). Under MSI-X the guest takes the event queue and RX queue on separate vectors, in either order: taking the stale RESPONSE after the reset sweep connects the socket to a muxer that no longer exists. MMIO's single interrupt took RX first, which is why MMIO never hung. r2 vs 517d438c5: no vsock or restore-path change beyond the MMIO removal; the earlier 10/10 PCI pass was luck.

Fix: Firecracker fxdax-v1.16.1-r3 (14dc26674; general commit bc3954c00 'vsock: a snapshot resets the connections in the RX queue, ahead of TRANSPORT_RESET', branch fxdax-v1.16.1-pci-vsock-reset): prepare_save kills the muxer's connections, each an RST in the RX queue behind any response, before TRANSPORT_RESET, bypassing the RX gate (every packet then is an RST); kick signals the RX queue too. Guest side: fxfs_automount::reconnect::exchange makes an exchange again on a new connection while a reset ends it (ECONNRESET, EPIPE, ENOTCONN, ECONNABORTED, EOF before the answer), used by fxfs-automount/git-remote-fxfs (idempotent asks) and the snapshot and git-fetch cases; docs/integrator-guide.md states the contract for integrators' agents. Verification: unfixed r2 hung in round 2 of 40 (and round 2 of 10 originally); r3 40/40 (t277b) and 30/30 (t277e, every round logged a reset made again: the race hit each round and was handled); restore cases 3 rounds each (t277d): all snapshot/suspend/migrate/git-fetch/shared-volumes pass, two unrelated failures filed (TASK-280 remount EBUSY before any snapshot, TASK-281 automount volume open after expiry); full run t277c: workspace + Firecracker KVM tests all pass, release guest suite 15/16 (TASK-257's flake). Series check: 15 commits, both ways (arm64 container). macOS workspace 2169/2169; scripts/upstream-changes.py --check passes.

CI at f824ffd50 (another agent's push on top of 96f1d068e): every job passed in at least one of runs 42579391459 (push) and 42581320255 (dispatched, so that the x86 firecracker-series check ran: passed). No single run is fully green: macos (workspace) in the first, macos (shuttle) in the second and its rerun, each failed installing tools (taiki-e/install-action, HTTP 504 from the download host for 60 s; TASK-285) before any test ran. Status left In Progress until a fully green run.
<!-- SECTION:NOTES:END -->
