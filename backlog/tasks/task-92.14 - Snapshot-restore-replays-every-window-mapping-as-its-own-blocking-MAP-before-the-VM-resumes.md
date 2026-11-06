---
id: TASK-92.14
title: >-
  Snapshot restore replays every window mapping as its own blocking MAP before
  the VM resumes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-22 21:06'
updated_date: '2026-11-06 04:00'
labels:
  - review-2026-10-21
dependencies:
  - TASK-191
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 289000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Restore replays the snapshot's window mappings one blocking MAP at a time before the VM resumes. Re-checked on main at 752515020 (2026-11-05): the VMM's ExtentDevice::map_again calls RemoteExtentStore::map_again for each recorded mapping in turn (vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs, map_again), inside activate_confined, which restore runs synchronously; each is a request_blocking MAP (crates/fxdax/src/extentd/client.rs), a round trip and a change of the window's second stage apiece. Since TASK-160.11 the window's mappings are one map (dax.rs WindowState) and since copy-on-write DAX (doc-13) a snapshot's window holds only sealed extents and zeros, read-only: a snapshot is refused (EBUSY) while the device has a private page, the guest's freeze writing them back and rebinding them first. So what restore maps again is exactly MAPPINGS' list of extents and zeros; no private page is restored as such, and the window starts with none. The cost is per mapping: a zygote with 500-1,000 mapped regions adds tens of ms to every restore, against a few ms for a plain Firecracker restore.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs (activate_confined, map_again), crates/fxdax/src/extentd/client.rs (map_again), crates/fxdax/src/extentd/server.rs (MAP), crates/fxdax/src/extents.rs (ExtentStore::map), crates/fxdax/src/dax.rs (Window::map_as).

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues'. Related: TASK-39.1, TASK-75, TASK-79.43, TASK-91.4, TASK-160.11, doc-13.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure first: a Linux unit test (extentd::tests) restoring a window of 1,280 mappings (1,024 one-page extents, 256 zeros) both ways, a MAP each and the new way, counting requests (daemon metrics) and stage operations and timing them; the guest snapshot suite given a file of 1,024 one-extent pages, mapped and stored to before the snapshot (private pages its freeze rebinds), each restore's /snapshot/load timed by the host. GCE r685 = main with the test change only (before).
2. Choose: batching into one request (MAP_MANY, the whole map in one operation, payload-bounded chunks) against lazily on fault. Lazy skips only the VMM's installs of pages already in memory (measured ~3.6 of ~15-20 ms) and moves each to a fault (a round trip, ~50 us) after resume; the daemon's per-mapping work stays either way. So: MAP_MANY.
3. Wire request MAP_MANY (31), payload as MAPPINGS' reply; codec Request::MapMany; daemon ExtentStore::map_many resolves every mapping, then maps them under one window lock in one change (Window::map_many: places all, tells the stage the ranges anew in runs, installs present pages in runs across mappings); client RemoteExtentStore::map_many (replaces map_again); Firecracker's map_again sends it once, logging count and time.
4. Tests: codec round trip and refusals; daemon fuzz (whatever_a_vm_process_says_harms_no_other) with MAP_MANY; the unit test of step 1 asserting 1 MAP_MANY and 0 MAPs; a unit test of private pages rebound then snapshotted and restored with the same bytes; the guest suite's pieces checked through the mapping, read(2) and the host's mount.
5. Verify: macOS workspace + shuttle, Linux fxdax in Docker, GCE r686 (after) of the five snapshot suites jailed, restore times compared with r685.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (TASK-92.14): a restore sends its whole window map in one new daemon request, MAP_MANY (wire 31, payload as MAPPINGS' reply), chunked by MAX_PAYLOAD (1.4M mappings a request). ExtentStore::map_many resolves every mapping as MAP would (the first failure fails the request, those before it mapped, each recorded in the window's change log as its own MAP), then maps them all under one window lock in one stage change: Window::map_many places each (Window::place, split from map_locked), tells the stage the ranges anew in runs, then installs the pages in memory in runs across mappings (Window::present + map_pages, split from install). The VMM's map_again calls RemoteExtentStore::map_many once and logs count and time at info. A new request rather than a MAP flag so an older daemon refuses it. Commits: ce1063d2a (fxdax + vendor/firecracker 0477d531b), 1cc1be207 (extentd says why a point is refused), 05c4b92a0 (guest/tests Cargo.lock).

Measurement (AC 1 and 2's measurement): extentd::tests::snapshots_map_their_windows_again_in_one_request, Linux Docker aarch64, 1,280 mappings (1,024 one-page extents written last first, 256 one-page zeros): before, a MAP each: 1,280 MAP requests, 2,560 stage operations, 60-68 ms (quiet) / 126-316 ms (loaded); after: 1 MAP_MANY, 2,048 stage operations, 14-15 ms (quiet) / 20-56 ms (loaded), of which the VM process's stage operations 3.6-5.5 ms. The rest is the daemon's per-mapping resolution, so restoring lazily on faults would save only the installs and pay each back as a fault after resume: measured and given up (dead-ends/lazy-restore-of-window-mappings.md; OPEN_QUESTIONS decision).

Private pages: a snapshot is refused while the window has a private page (doc-13), so a restore never maps one. snapshots_after_private_pages_are_rebound_restore_their_bytes covers the path: stores make private pages, the point is refused (EBUSY), they are written back and rebound, the point is made, the restore's MAP_MANY reads the stored bytes. The guest end to end version (a 1,024-page DAX file, one extent a page, mapped and stored to, frozen, snapshotted, restored; its restores timed) found TASK-191 on main: the host keeps 1-2 private pages its frozen guest doesn't count, and refuses the point, even with no store, after reads through a shared mapping (GCE r685 to r689: the four DAX snapshot suites fail most rounds, before and after this change alike). That test is held back, not landed, and is TASK-191's reproduction; it lands with TASK-191's fix (its last version, read-only, in TASK-191's notes). Overlap: dax.rs's Window (map_locked/install split) is the DAX fault-handling work's area; metrics.rs's request kinds the stats work's (TASK-111.4/5); TASK-191 is in the host DAX fault path TASK-187 changed.

GCE (n2-standard-4, nested, jailed, FXDAX_FIRECRACKER_LOG_LEVEL=Info; us-east4 out of quota for r685, r687-r689, which ran in us-east1-b): r685 (main 752515020 + the guest test with stores, before): fxfs-guest-snapshot 3/3; DAX suites fail by TASK-191 but one round of migrate-tenant-dax, restores 186-237 ms (four at once, a MAP each). r686 (this change + that test): fxfs-guest-snapshot 3/3 (restores 35-59 ms, as before: no window); migrate-dax round 1 passed, restore 46 ms through MAP_MANY, bytes checked; the rest TASK-191. r687 (fxfs-guest-snapshot-dax, 1 round, the counts at the freeze): passed, two zygote restores through MAP_MANY of ~1,280 mappings, bytes checked. r688 (4 rounds each of snapshot-dax and suspend-dax): the guest's counts 0/0 after the freeze, the daemon's 'the window has 2 (1) private pages'. r689 (all five, 3 rounds, the read-only test): fxfs-guest-snapshot 3/3; every DAX round refused by TASK-191, with no store. A like-for-like before/after GCE restore time on DAX needs TASK-191 fixed.

Blind spot (AGENTS.md): no suite snapshots a VM whose window maps what its guest read through a mapping: the snapshot suites' only mapped file is created in the mount, a page-cache file. TASK-191 carries the test that closes it.

Other verification: macOS FXDAX_TEST_PARTS='clippy workspace shuttle' green (workspace 2118/2118); Linux Docker fxdax 481/485 (builds_are_stamped_with_their_commit and three grants tests, Docker's own, as main), shuttle 59/59, fork/snapshot/fuzz set 84/84 after the logging change; whatever_a_vm_process_says_harms_no_other timed out once under an overloaded host (322 s, a compile OOM-killed beside it), then passed 3/3 alone; vendor/firecracker cargo check -p vmm --tests on Linux; guest/tests test_cases checked both halves; scripts/upstream-changes.py --check passes.

Not done: AC 2's GCE suites passing (the DAX snapshot suites fail on main by TASK-191, with or without this change), and DoD 1's Lima run (Linux ran in Docker via OrbStack, as instructed). Left In Progress, depending on TASK-191.

Correction: r687's mapping count wasn't recorded (a passing round keeps no VM log); its window mapped the 1,024 pieces plus the files' metadata, so 'about 1,280' is an estimate.
<!-- SECTION:NOTES:END -->
