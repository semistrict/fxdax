---
id: TASK-56
title: >-
  VMM: a blocking store call on the thread receiving extentd's replies panics
  the VM process
status: To Do
assignee: []
created_date: '2026-10-21 10:36'
labels:
  - extentd
  - firecracker
  - reliability
dependencies: []
priority: medium
type: bug
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Since 93a73fc (TASK-41), Firecracker's extent device completes requests on the thread that receives extentd's replies (crates/fxdax/src/extentd/client.rs, spawn_on_replies; vendor/firecracker .../extent/worker.rs). A blocking store call on that thread would wait for a reply only that same thread can receive, and deadlock. Instead the assert! in client.rs's blocking() helper panics the Firecracker process, which kills that sandbox's VM. It fires only on a bug in fxdax's own code (a callback or continuation running on the receiving thread that calls a blocking RemoteExtentStore method), not on anything the guest does; but it is a second way a VM can die while extentd is fine, besides the daemon going (TASK-50.2). The assert is there to fail fast rather than hang. Options: prove statically that nothing on the receiving thread blocks (the async API alone reachable there), or degrade instead of panicking (fail the request with EIO, log it, keep the VM), or both.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every path that runs on the receiving thread (spawn_on_replies continuations, on_capacity, on_committed, on_daemon_lost callbacks, stage operations) is shown to make no blocking store call, by a test or by construction
- [ ] #2 A blocking call made there anyway fails that request (EIO) and logs it, rather than panicking the VMM process, with a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
