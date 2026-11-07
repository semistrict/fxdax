---
id: TASK-231
title: >-
  A VM whose daemon dies with a DAX fault in flight dies of SIGABRT, not
  ExtentdLost (158)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 11:54'
updated_date: '2026-11-07 11:54'
labels:
  - bug
dependencies: []
priority: high
ordinal: 665000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67), GCE r830 round 27, file bucket, jailed (run FXDAX_SEED=14315020714128627110, round seed 3630852399382050444; plan: slot 2's pgbench VM killed at 38 s and booted again, the daemon killed at 47 s). The rebooted VM's process ended by a signal, not 158 as doc-12 promises a VM whose daemon goes (the README's exit statuses, what a VM manager tells a daemon's loss from the VM's own by). Its log: `receiving from the extent daemon: Connection reset by peer`, then `unresolved DAX window fault at offset 0x2128000 (Write): EIO`, and nothing more. A fault the window fails aborts the process (crates/fxdax/src/dax/uffd.rs:1373-1381, std::process::abort), as its faulting thread can't go on; and the client's reader, as the connection ended, failed the requests waiting before it marked the daemon lost (crates/fxdax/src/extentd/client.rs:950-961): a fault in flight failed EIO, looked like a refusal, and its abort beat the device's exit 158 on losing the daemon.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A fault failed as its daemon goes waits for the VM process to stop rather than abort it, shown by a test that fails first
- [ ] #2 What a request its daemon's going failed does finds the daemon lost, shown by a test that fails first
- [ ] #3 fxfs-guest-soak runs daemon kills with VMs faulting without a VM dying of a signal
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Tests that fail first: a fault whose window fails as its daemon goes doesn't abort (uffd, a fake window: SIGABRT before); a request the daemon's going failed finds the daemon lost (a daemon by hand, a failpoint holding the connection's end between failing the requests and the rest).
2. Fix: the client marks the daemon lost before failing what waits; Faults::lost; a fault failed with the daemon lost waits for the VM process's exit.
3. Linux Docker, macOS, CI; the soak's daemon kills.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed on land-k4. client.rs: the connection's end marks the daemon lost before the requests waiting fail, the log and the on_lost callback after (failpoint client::requests_failed between). dax.rs: Faults::lost (false by default), RemoteFaults's the client's. uffd.rs resolve_through: a fault failed while the window is lost logs a warning and waits for ever (std::future::pending), the VM process stopping, ExtentdLost; any other unresolved fault aborts as before.

Tests: dax::uffd::tests::faults_failed_as_their_daemon_goes_wait, SIGABRT before, green after; extentd::tests::requests_failed_by_the_daemons_going_find_it_gone, red before with the failpoint sleeping 200 ms (300 of 300 passed without it: the race needs the reader to lag), green after; Linux Docker (privileged, for userfaultfd): those and every dax::uffd and daemon test, 27 of 27; fxdax's handover tests (a real daemon killed) 3 of 3; fxdax 311 of 311 on macOS.

Blind spot: the daemon-lost tests kill the daemon with the store idle (handover.rs's stores_are_told_when_their_daemon_goes, fxfs-guest-daemon-lost, crash-dax-daemon-killed, whose VMs' faults happened not to be in flight); none had a fault in flight as the connection ended, nor asked what a failure saw of the loss. Looking around: the window's other request paths fail their callers with EIO and leave it to them (the device's requests, reads, writes, commits), and the VMM's device exits 158 on the loss; only the fault thread turned EIO into the process's end. Structure: the loss was said after its effects; one flag set first, which every failure can consult, makes the order impossible to get wrong again.
<!-- SECTION:NOTES:END -->
