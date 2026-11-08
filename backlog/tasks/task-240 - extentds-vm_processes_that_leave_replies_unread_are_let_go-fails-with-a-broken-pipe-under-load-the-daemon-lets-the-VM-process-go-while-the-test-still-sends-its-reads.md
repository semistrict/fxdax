---
id: TASK-240
title: >-
  extentd's vm_processes_that_leave_replies_unread_are_let_go fails with a
  broken pipe under load: the daemon lets the VM process go while the test still
  sends its reads
status: To Do
assignee: []
created_date: '2026-11-08 03:21'
labels:
  - testing
dependencies: []
references:
  - crates/fxdax/src/extentd/tests.rs
  - crates/fxdax/src/extentd/server.rs
priority: medium
type: bug
ordinal: 675000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen 2026-11-08 in TASK-234's Linux run (Docker on macOS, rust:1, privileged, cargo nextest run -p fxdax, alongside a GCE run's local work and the macOS workspace's build): one failure, panicked at crates/fxdax/src/extentd/tests.rs:3602:37, `called Result::unwrap() on an Err value: Os { code: 32, kind: BrokenPipe }`. Run alone in the same container it passed 8 of 8 times; it passed in the run before too.

The race, as the code reads (unconfirmed by a trace): the test (TASK-79.16) writes 8 MiB and then sends 64 READs of it, 512 MiB of replies, in a loop, unwrapping each send (tests.rs:3600-3603), and only after the loop polls for the daemon's hang-up (tests.rs:3605-3609). The daemon lets a VM process go once what it leaves unread passes what a connection may hold ("kept more in flight than the protocol allows", server.rs:480-484, peer.let_go, which shuts the socket). Under load the daemon can serve the first reads and pass that bound before the test's loop has sent the rest, so a later send meets the closed socket: EPIPE at tests.rs:3602. The hang-up the test then looks for has already happened; only its own send fails.

Likely the same in vm_processes_naming_vast_windows_are_let_go_all_the_same, whose loop sends reads the same way (tests.rs:3633).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test sends its reads tolerating the daemon's letting it go meanwhile (a send failing EPIPE or ECONNRESET ends the loop), and still checks the hang-up: what the daemon does, not when
- [ ] #2 The same for vm_processes_naming_vast_windows_are_let_go_all_the_same if its loop can race the same way
- [ ] #3 Each passes under load, e.g. cargo nextest run -p fxdax with other builds running, or nextest's stress mode, many times over
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
