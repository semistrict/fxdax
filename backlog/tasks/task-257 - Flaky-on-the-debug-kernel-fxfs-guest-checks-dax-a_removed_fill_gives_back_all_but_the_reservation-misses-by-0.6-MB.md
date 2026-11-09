---
id: TASK-257
title: >-
  Flaky: fxfs-guest-checks a_removed_fill_gives_back_all_but_the_reservation
  ignores the layer files and journal a flush during the fill changes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 10:38'
updated_date: '2026-11-09 09:13'
labels:
  - bug
  - flaky
dependencies: []
priority: high
ordinal: 690000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE validation run B (2026-11-08, main 08d444ef7, debug kernel: KCSAN, lockdep; fxdax-keep-n2-standard-4-us-west1-b-1), seed 1794336019930021975: 'a_removed_fill_gives_back_all_but_the_reservation: 268435456 bytes, 242262016 free before, 254799872 after, -12537856 short; reserved 13128828, then 3907564' then attributes.rs:201 '-12537856 short, the reservation grew -9221264': short - grew = -3.3 MB against a tolerance of size/100 (2.68 MB). The check came with TASK-149.25 (ffc85acac), green on the release kernel (r945-r947). Likely other metadata (compaction's layer files) freed during the fill on the slower debug kernel, which the check counts against the reservation; unconfirmed. Results: the run's e2e/fxfs-guest-checks-dax log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found: what besides the metadata reservation changes free space across the fill and removal
- [ ] #2 The check holds on the debug kernel for 20 rounds, measuring what it means to, its tolerance not loosened to hide a cause
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce on the host: fxfs-posix test on a 256 MiB fake device with the guest's journal options, the journal filled to a 12 MiB reservation before the fill.
2. Divide the allocator's used bytes (Allocator::usage: held by owner, pending, uncommitted, reserved) and find what besides the reservation changes free space.
3. Export the division in fxfs-linux's stats; check the model in the guest check: data held back but for the staging extent, the rest Fxfs's metadata held and reserved.
4. GCE: debug and release kernels, the four fxfs-guest-checks variants, 20+ check runs each.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: failed on the release kernel too (GCE fxdax-test-r1001-t1492, n2-standard-16, fxfs-guest-checks-dax seed 1794222176066265452, at the tracepoints commit ad266aff6's parent tree plus it): 268435456 bytes, 240873472 free before, 253026304 after, -12152832 short; reserved 14139932, then 5202908 (the reservation grew -8937024). It passed in the same commit's first release run and its debug run.

2026-11-08, TASK-269 GCE debug round (run-20261108T233114Z-d2a31963, vendor/firecracker fxdax-v1.16.1-r1): failed again in fxfs-guest-checks (seed 1793593445840806806) and fxfs-guest-checks-dax-small (seed 1793647760234740270), same check; fxfs-guest-checks-dax and -dax-delay passed.

2026-11-09: seen on the release kernel too, over virtio-PCI (TASK-267 GCE pcirel5, seed 1794651830648838433): 268435456 bytes, 242372608 free before, 254468096 after, -12095488 short; reserved 13202664, then 4308976 (the reservation grew -8893688). Passed in pcidbg4 and every other PCI round.

2026-11-09, again on the debug kernel: GCE t273dbg (TASK-273, main 0f011561a's code, fxdax-keep-n2-standard-4-us-east4-a-1), fxfs-guest-checks-dax-delay seed 1794564303531441755: 268435456 bytes, 242634752 free before, 254599168 after, -11964416 short; reserved 12993740, then 4171088; the reservation grew -8822652: short - grew = -3.1 MB against the 2.68 MB tolerance, the same as before. Every device on virtio-PCI there; the release round's 16 cases passed.

Seen again 2026-11-09 on the release guest kernel, fxfs-guest-checks-overrun (seed 1793764864622820584): 'FAILED: crashes, hangs, a_removed_fill_gives_back_all_but_the_reservation' where 'crashes, hangs' was expected; GCE n2-standard-4 us-west1-b, TASK-277's run t277c (fxdax 9d4676dd5, Firecracker 14dc26674); every other case of the release suite passed (15/16).

Again on the debug kernel, 2026-11-09, TASK-276's debug round (GCE mqdbg, fxdax-keep-n2-standard-4-us-east4-a-1, queue per vCPU + affine queue interrupts + prefault behind): 'a_removed_fill_gives_back_all_but_the_reservation: 268435456 bytes, 242720768 free before, 255148032 after, -12427264 short; reserved 12762832, then 3603632'; every other case of the round passed.

2026-11-09, TASK-276 re-run of fxfs-guest-checks-dax alone on the debug kernel, 3 rounds asked (GCE mqdbg2): round 1 passed, round 2 failed the same check (seed 1793061914577705328), the run stopping there. With the earlier occurrences (main's release and debug kernels, MMIO and PCI, ~12.1-12.5 MB short each time) it is TASK-257's, not the queue changes'.

Root cause (measured, AC #1): the check's model, not Fxfs. Free space = device - (extents alive by owner + pending + uncommitted + reserved). Across the fill and removal, besides the metadata reservation, these change it: (a) the trees' layer files, held by the root store (volume and allocator layers) and the root parent store (the root store's): with the journal near its reclaim size (the checks before leave the reservation at ~13 MB), a flush during the fill writes them as the reservation shrinks; in the guest it compacted, freeing old layers: metadata held -3.2 MB with the reservation -9.0 MB, so free space came back 12.2 MB *more* than before (the failures' 'short' was negative: -12.5 MB short, i.e. more free), short - grew = -3.2 MB; on the host's first flush, layers +3.7 MB against reservation -3.5 MB; (b) the journal file (root parent store), +1.0..1.2 MB in each fill; (c) the volume's open staging extent, counted as far as it has reached with nothing allocated in it, 0..0.77 MB (at most STAGING_SIZE, 2 MiB). The fill's data all comes back. Host repro: crates/fxfs-posix tests.rs a_removed_fill_gives_back_all_its_data (old model misses by 5.1 MB there).

Fix: Allocator::usage() (fxdax, one lock) divides used bytes; fxfs-linux stats add used_bytes, metadata_held_bytes (root parent + root store owners), data_held_bytes (other owners + pending), reserved_bytes, uncommitted_bytes. The guest check, renamed a_removed_fill_gives_back_all_its_data, asserts data held grew by no more than STAGING_SIZE (tighter than the old 2.68 MB), and short == data + metadata held + reservation deltas within the old size/100. Measured identity in the guest within a few KB.

GCE (fxdax-keep-n2-standard-4-us-west1-a-1): debug t257dbg1+t257dbg2: the check passed 34/34 runs across fxfs-guest-checks, -dax, -dax-small, -dax-delay, 8 with the flush (the old failure's shape). Rounds lost to other bugs: dax-small KCSAN (TASK-286), dax-delay reclaim deadlock (TASK-292). Release t257rel1: 20/20 rounds of the four variants passed (min short -12656640, max 4202496). macOS scripts/test.sh passed (2170 tests).
<!-- SECTION:NOTES:END -->
