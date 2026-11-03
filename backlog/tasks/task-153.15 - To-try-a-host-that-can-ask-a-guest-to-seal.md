---
id: TASK-153.15
title: 'To try: a host that can ask a guest to seal'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-02 01:59'
updated_date: '2026-11-03 12:36'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 563000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-150.20: only a guest can seal, and the daemon can neither evict open pages nor ask, so one idle VM open pages can use the shared budget and another VM gets ENOMEM, when several run. Options: a device event asking a guest to seal, a per-tenant share of open pages, or evicting open pages to the store. Not run with several VMs at once.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test with several VMs sharing a budget, one idle
- [x] #2 Decided, from what it shows
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Device: F_SEAL, the config's seal_asked (0 if none), T_SEALED answering it; daemon->VMM SEAL, VMM->daemon SEALED on the wire. 2. Daemon (extentd/seals.rs): a writable MAP refused for want of open-page budget asks the guests of other writer connections holding the most open pages (ExtentStore::open_bytes), its tenant's first, to seal, and waits for their answers (bounded, SEAL_WAIT), holding no thread, then retries once. A connection asked to seal never waits (no deadlock cycle); one that leaves an ask unanswered past SEAL_WAIT isn't waited for again until it answers. 3. VMM: Sealing in the device's Notices; forwarded to a driver that acked F_SEAL, answered at once otherwise, and at reset. 4. Guest: driver reads seal_asked; Fxfs's config-change callback schedules a seal work: fxfs_seal (journal sync, an ordered commit) and SEALED; seals_asked in /proc stats. 5. Tests: host unit tests (seals.rs policy, extentd two-connection test, VMM Sealing), red with each rule out; guest test fxfs-guest-host-seal (two VMs, one idle holding all but a write's room), red on the base. 6. Proof: scripts/test.sh, GCE all with the new case and DAX suites plus the Linux workspace, upstream-changes --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
DESIGN: a device notice shaped like the resize one. doc-4's F_SEAL (9): the config's seal_asked holds the number of the host's ask, 0 if none, and changes with a config interrupt. The guest answers with T_SEALED (15). Wire: SEAL (107) goes daemon to VM process, SEALED (29) comes back. VMM: Sealing in the device's Notices, a refactor of its own commit that also carries resizing and frozen. An ask reaches a driver that acked F_SEAL; otherwise the client answers at once. At a reset the device answers for its guest. Guest: Fxfs's config-change callback (fxfs_device_set_changed, renamed from set_resized) schedules a seal work, which reads seal_asked. If it is not 0 the work calls fxfs_seal, which syncs the journal with an ordered commit, then answers SEALED. The work also runs as the filesystem closes. seals_asked is in /proc/fs/fxfs/<tag>.
WHEN THE DAEMON ASKS (extentd/seals.rs): only under pressure, when a writable MAP is refused. It asks the other writer connections holding the most open pages (ExtentStore::open_bytes), its tenant's arena first, enough to cover the MAP. It waits for their answers as a future (serve_future), holding no thread, SEAL_WAIT (10 s) at most, then tries once more. Refused again, the MAP says ENOMEM and the guest seals itself, as in TASK-150.20. No idle timer: the ranking already picks the idle VM, and a timer would seal VMs nobody needs room from. NO DEADLOCK: a connection whose guest has been asked never waits for another's seal (its own seal may wait for its MAP in the gate). Checking and asking happen under one global lock, so whoever is waited for began waiting before it was asked, and no cycle closes. BOUNDED: a guest that leaves an ask unanswered past SEAL_WAIT isn't waited for again until it answers. Unanswered asks count as one.
TESTS: extentd::tests::maps_refused_ask_the_guests_holding_open_pages_to_seal: two connections, a 16-chunk budget. Red with the ask removed ('the idle guest asked: Timeout'), and red with the no-wait-when-asked rule removed ('it waited 10.0s'). asks_to_seal_that_nothing_takes_are_answered_at_once. Unit tests: seals.rs (choose, one ask at a time, asks not told, guests that don't answer); VMM asks_to_seal_reach_drivers_that_take_them; cache open_pages_are_pinned_until_they_go and extents writable_maps_past_the_open_budget_are_refused_until_a_seal now also check the count. Guest: fxfs-guest-host-seal. The idle VM fills until the daemon refuses a MAP, then seals and rewrites fewer each time until nothing is refused: it holds all but less than a write's room. Then the busy VM writes 64 MiB. On the base tree (GCE): 'the idle guest is holding 112 MiB', busy 'write 0: Out of memory (os error 12)'. With the change it passes, and the daemon logs 'waits for 1 other guests asked to seal'.
RESULTS: macOS scripts/test.sh: 2044 of 2044 plus the Shuttle and script suites. GCE fxdax-test-t15315-after (deleted): Linux workspace 2214 of 2214 plus Shuttle; fxfs-guest-host-seal, fxfs-guest-host-memory, fxfs-guest-dax and fxfs-guest-checks-dax passed. VMM extent tests (Lima, --features fxdax): 12 of 12. upstream-changes.py --check passes.
BLIND SPOT: nothing ran two VMs against one daemon's memory. Every memory test had one guest, so another guest's open pages never held the budget. The host-side test now plays two connections. STRUCTURE: the class (memory only one guest can release) stays; asking narrows it to guests that don't answer, which are bounded and then skipped. Evicting open pages to the store (option 3) would close it, at the cost of taking writable pages from a running guest. dead-ends/map-waits-for-a-seal.md updated: its 'reopen if' (ask the guest to seal) came about, and the MAP still never waits for its own guest.
UNVERIFIED: a paused or frozen guest asked to seal (the bound is unit-tested only); many VMs at once; the guest sim auto-stubs the new driver calls and plays no ask.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
When a writable MAP is refused, the daemon asks the guests holding the most open pages to seal, through a new device notice (doc-4's F_SEAL, seal_asked, SEALED). It waits for their answers, 10 s at most, and tries once more; a guest that has been asked never waits for another's seal. Proved by fxfs-guest-host-seal: two VMs, one idle holding the budget. Without the change the busy VM's first write fails ENOMEM; with it, the test passes on GCE, as do the DAX suites and the Linux workspace (2214 of 2214) and macOS (2044 of 2044). Host unit tests are red with the ask removed and with the no-wait rule removed. Decision in OPEN_QUESTIONS.md. Unverified: a paused guest asked to seal, beyond unit tests.
<!-- SECTION:FINAL_SUMMARY:END -->
