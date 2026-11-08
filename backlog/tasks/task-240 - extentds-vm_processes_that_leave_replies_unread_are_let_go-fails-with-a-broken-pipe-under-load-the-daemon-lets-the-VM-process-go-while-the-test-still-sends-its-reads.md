---
id: TASK-240
title: >-
  extentd's vm_processes_that_leave_replies_unread_are_let_go fails with a
  broken pipe under load: the daemon lets the VM process go while the test still
  sends its reads
status: Done
assignee: []
created_date: '2026-11-08 03:21'
updated_date: '2026-11-08 05:59'
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
- [x] #1 The test sends its reads tolerating the daemon's letting it go meanwhile (a send failing EPIPE or ECONNRESET ends the loop), and still checks the hang-up: what the daemon does, not when
- [x] #2 The same for vm_processes_naming_vast_windows_are_let_go_all_the_same if its loop can race the same way
- [x] #3 Each passes under load, e.g. cargo nextest run -p fxdax with other builds running, or nextest's stress mode, many times over
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A helper sending the reads that ends at a send failing EPIPE or ECONNRESET (send_until_let_go), used by both tests. 2. A helper asserting the let-go: POLLRDHUP within 10 s, and a send after it failing EPIPE/ECONNRESET (assert_let_go). 3. Stress both under load in Linux Docker, before and after.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 (closure sweep): implemented in 728aac974 on land-m1 (CI run 42447440838), not yet on main. Reproducing the race before the fix: not done: in Linux Docker (aarch64, 15 CPUs), 300 runs of both tests beside 14 CPU-bound loops and 8 x 100 runs side by side, all 1,100 passed with the old sends. The mechanism is shown deterministically instead: assert_let_go sends once the daemon hung up, and that send fails EPIPE, which is what the old unwrap at tests.rs:3602 met when a send came after the let-go. vm_processes_naming_vast_windows_are_let_go_all_the_same's loop raced the same way (AC #2) and takes both helpers. After: both pass, 8 concurrent stress runs of 100 each, 800 of 800, Linux Docker. Left for the landing: AC/DoD after land-m1's CI, then Done.

2026-11-08: landed with land-all4 as 17c9c5e91 (was 728aac974); green main CI 42449438764 (land-all4, at 2b19077be, main 722ce0723), whose macOS and Linux workspace jobs contain it. DoD #1 rests on that CI run, not GCE.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Both let-go tests send their reads through send_until_let_go, which ends at EPIPE or ECONNRESET, and check the let-go with assert_let_go: POLLRDHUP, then a send after it fails EPIPE, which is what the old unwrap met. The race wasn't reproduced before the fix (1,100 loaded runs passed); the final send shows its mechanism deterministically. Verified: 800 of 800 under 8 concurrent stress runs in Linux Docker; green main CI 42449438764 (land-all4, at 2b19077be, main 722ce0723), whose macOS and Linux workspace jobs contain it.
<!-- SECTION:FINAL_SUMMARY:END -->
