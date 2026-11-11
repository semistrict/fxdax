---
id: TASK-343
title: >-
  Hunt: extentd hostile finds mallory's disk past its limit plus a MiB (by 4 KiB
  to 1 MiB), on main since TASK-92.41
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 07:26'
updated_date: '2026-11-11 15:13'
labels:
  - extentd
  - testing
dependencies: []
priority: high
type: bug
ordinal: 738000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Nightly Hunt run 42949975683 (main aaba26d2b, 2026-11-11) failed 3 of 4 'extentd hostile' shards at crates/fxdax/src/extentd/tests/hostile.rs, the TASK-94 disk check (counted <= limit + 1 MiB):
- 202690560 bytes against a limit of 200556544 (2.03 MiB over the limit, 1.04 MiB past the allowed MiB)
- 593981440 against 592928768 (4 KiB past the allowed MiB)
- 1181257728 against 1180200960 (8 KiB past)
The previous Hunt, 42746736395 on 94f63b137 (2026-11-10), passed. Between them came TASK-92.41: commits spanning segments, filled segments waiting for the next commit, and segments framed in place (raw file plus an appended tail). The likely cause: a store's own work, meaning segments held open or unframed and framing tails, now adds more than the MiB TASK-94 allows beside what was admitted. Either the bound is wrong under the new design, or the extra is not counted or admitted. Proptest's shrunk cases are in the log.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Root cause shown: which of the store's files take the excess, and why
- [x] #2 Fixed in the store or in TASK-94's accounting, with the bound justified in hostile.rs, not just widened
- [x] #3 The failing cases replayed deterministically as a regression test
- [x] #4 Hunt's extentd hostile shards pass at 160 times their cases
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Replay Hunt run 14's hostile seeds on ubu1 on tmpfs, with the disk check naming the files grown; find the file and why it was kept raw; fix extent_store's saves_little; regression tests at segments.rs and store level; justify hostile.rs's MiB; rerun seeds 53-56 at 1920 cases.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause (AC #1). Replayed Hunt run 14's seeds (seed = (14-1)*4 + shard + 1: 53, 54, 55, 56) at FXDAX_HOSTILE_CASES=1920 on ubu1, under fxjob, with TMPDIR on tmpfs, which counts blocks at once as Hunt's ext4 does. Seed 53 failed at case 283: mallory's disk at 19611648 against a limit of 17981440. The disk check, which now names the files grown since the case began (hostile.rs files_taking/grown_since), named one segment, mallory/283-2/segments/<branch>/1, +5795840 bytes: about 3 MiB of the fills' noise, then about 2.6 MB of zeros. The zeros are the holes of sparse extents, which the commit seals in one run with them (TASK-92.41), so the store writes them itself and no admitted write paid for them. The segment was framed in place, raw, because segments.rs saves_little sampled one 1 MiB part in eight. For a segment under 9 MiB that is part 0 alone, which here was noise, so the sample said compressing saves little. Framed in a copy, compressed, the holes would take nothing.

Fix (AC #2). saves_little counts zero 4 KiB blocks exactly. If they reach len/16 (half of IN_PLACE_SAVING's eighth), it goes to whole compression, which frames them in a copy. Otherwise it samples an eighth of every part (SAMPLE = PART/8, one piece in eight), so no part goes unsampled. hostile.rs's bound comment now justifies the MiB: in-place framing keeps a segment raw only if compressing it saves under an eighth of it. Its holes are then under an eighth of it, under a seventh of the admitted rest, which is under 4/7 MiB of a case's FILL_ROOM. Logs, branch files and tails take tens of KiB.

Regression tests (AC #3).
- extent_store segments.rs: segments_holding_holes_save_much_compressed (noise then zeros; zeros where the old sample couldn't reach; noise; noise with a few zero blocks).
- tests/store.rs: segments_of_noise_then_holes_are_framed_compressed. Three 1 MiB extents of noise, then three extents of one block at their ends (1 MiB holes each) in one 8 MiB-segment store, closed and committed. The segment is not framed in place, and every extent reads back as written, before and after a crash. Both fail with the old saves_little (framed in place) and pass with the fix.
- An extentd-level test that only wrote the case's shape did not reproduce: by the disk check, the closed segment had been uploaded and was gone from disk.

Other findings.
- On ZFS (ubu1's home), seed 53 instead failed at case 42: a 6 MiB raw noise segment was admitted with 4 MiB of room, because recounts read st_blocks before the txg sync. Filed as TASK-355.
- Seed 54 on ZFS failed at case 907 with "mallory's store didn't commit", and seed 55 on tmpfs at case 811 the same way: the commit's status was None. Being replayed before TASK-92.41 (94f63b137) to tell whether it predates it.

Pre-TASK-92.41 replay: seed 55 on 94f63b137 (tmpfs) passed all 1920 cases (1473 s). The hostile strategies didn't change between 94f63b137 and main; only the heap bound moved to crate::heap.

Validation with the fix (t276x-343, b95832a0), all on ubu1 tmpfs under fxjob --mem 8G, FXDAX_HOSTILE_CASES=1920, PROPTEST_MAX_SHRINK_ITERS=0: seed 53 passed (1785 s), seed 56 passed (1815 s), seed 55 passed (1752 s), seed 54 passed (2040 s).
- A first run of seeds 54 and 55 together failed both at once, about 1420 s in, with "a connection of mallory's never ended: Timeout", as ubu1's load went from about 4 to 35. Run one after the other on a quieter host, both passed.
- The earlier "didn't commit" failures (seed 55 case 811 on main with debugging, seed 54 case 907 on ZFS) didn't recur. Filed as TASK-357, with the timeouts.
- extent_store: cargo nextest run -p extent_store passed on macOS, 226 tests. segments_of_noise_then_holes_are_framed_compressed fails with the old saves_little ("framed in place") and passes with the fix.
<!-- SECTION:NOTES:END -->
