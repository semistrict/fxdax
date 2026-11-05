---
id: TASK-189
title: >-
  extent_store test segments_hold_a_segment_and_an_extent_at_most reads a framed
  file its framing hasn't written yet: Corrupt("an object's trailer")
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 21:47'
updated_date: '2026-11-05 23:16'
labels:
  - extent_store
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 642000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42105764695 (land-i at 9f283fdbc, main ce9d5b2f2 plus TASK-184's fxdax-only commits): extent_store::store segments_hold_a_segment_and_an_extent_at_most panicked at crates/extent_store/tests/store.rs:2236, extent_frame::data(...) of a file in segments/ gave Corrupt("an object's trailer"), the error for bytes shorter than a trailer. 2,331 of 2,332 passed; main's own CI passed the test on ce9d5b2f2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test red now reads the store's segments the way segment_data does while a framing has created its framed file and not yet written it
- [x] #2 The test's helper reads no framed file whose framing is running, and the test passes; stressed on macOS and Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rule out TASK-184 (fxdax only) and TASK-165.5 (extent_remote's WAL) by what extent_store's test binary is built from.
2. Read frame_segment and the helper: the framed file is created, then written, on a thread of its own.
3. Stress the test on land-i and origin/main, macOS and Linux (Docker).
4. Failpoint segments::framing between the create and the write; Store::framing names the segments being framed; segment_data skips their framed files. Red: the helper run at the failpoint, unfixed. Green: fixed.
5. Rebase onto origin/main; extent_store's suites, the workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Not TASK-184, not TASK-165.5
- land-i's commits over main touch crates/fxdax and backlog only; fxdax isn't in extent_store's dependency tree (cargo tree -p extent_store -e normal,dev,build), so the test binary doesn't contain them.
- extent_store uses only extent_remote::recovery from extent_remote; TASK-165.5's WAL changed batch.rs, wal.rs, writer.rs, lib.rs and the sim, none of which extent_store reaches. crates/extent_store, extent_frame, extent_remote, storage_device and Cargo.lock are byte-identical on origin/main and origin/land-i.

## Cause: the test reads a file the store is still writing
frame_segment (segments.rs) creates the framed file segments/<n>, then writes it, on a thread of its own when the directory is concurrent (MemDir::new, HostDir), behind the write or commit that closed the segment (Store::frame_behind). segment_data, the test's helper, lists segments/ after each write and frame-parses every file: one created and not yet written is empty, and extent_frame::data gives Corrupt("an object's trailer") (Frame::of_trailer: shorter than a trailer). A write closing the open segment for room, the segment holding a commit's data, frames it behind with nothing waiting for it. A commit's flush waits only for the segments its own commits closed. The store itself reads a framed file only once its framing ends, and opening frames anew one that isn't whole (close_left_open, TASK-79.33): a product reader never sees one. The race is the test's alone.

## Reproduced (2026-11-05)
- macOS (24 at once, 8 busy loops): land-i 0 of 2,000 idle, 0 of 4,800 under load.
- Linux, Docker rust:1 on this Mac (aarch64, 15 CPUs, 24 at once, 8 busy loops): origin/main 1 of 4,800, then 5 of 4,800 with the panic kept: store.rs:2236:75, Corrupt("an object's trailer"), CI's own. land-i 3 of 4,800.

## Fix
- Store::framing: the numbers of the branch's segments being framed. segment_data skips their framed files; their raw files, still in open-segments/, hold their data and are measured.
- churn_pinning (tests/store.rs) frame-parsed every new segments/ file after each commit too: it skips those being framed and counts them once framed.
- Failpoint segments::framing in frame_segment between the framed file's creation and its write; extent_store's tests set failpoints (dev-dependency fail with failpoints).

## Red, green
segments_read_beside_the_store_skip_framed_files_being_written runs segment_data at that failpoint, on the framing's thread, for each segment a write closes and frames behind. Red with the skip out (macOS): two reads, both Corrupt("an object's trailer"). Green with it. extent_store's suite on macOS: 226 of 226.

## Blind spot and structure
The tests that inspect a store's files beside it took every file in segments/ for a whole framed segment, which the store never promised while framings run on other threads. Nothing ordered the reader with the framing thread, so the race needed load and timing (Linux under load, about 1 in 1,000). The store already knows which framings run; readers now ask it. Writing the framed file to a new name and renaming it whole would make segments/ hold only whole files. But Dir has no rename, MemDir's crash model would need one, and TASK-79.33 already tolerates a torn framed file: not worth it for a test's view. The other readers of segments/ in tests/store.rs (lines 1399, 1801) read after a commit's flush, or from Closed, both after their framings: not exposed.

## The regression test, redone
Its first version ran segment_data on the framing's thread, at the failpoint, while the test thread kept writing. Linux stress (Docker) failed it 3 of 4,800: a later write began a framing after the helper's snapshot of Store::framing, and its empty file wasn't skipped. That interleaving is the test's own; real callers read between writes, on the writing thread. Now each framing waits at the failpoint, its file created and empty, while the test thread, between writes, waits for every framing the write began, runs segment_data, and lets them go. Red with the skip out (macOS): Corrupt("an object's trailer") on the test thread, CI's error. Green with it. The callback ignores closed channels, so a failing run ends rather than leaving the store's drop waiting on framings that wait forever.

## One process for all store.rs tests (cargo test)
Not supported, on main either: on origin/main's Linux binary run in one process, parts_read_again_are_unpacked_once fails 3 of 3. The failpoint, being the process's, also assumes nextest's process per test, which CI and scripts/test.sh use.

## Verified at the final head (on origin/main ef8223900)
- macOS, 24 at once with 8 busy loops: segments_hold_a_segment_and_an_extent_at_most, segments_read_beside_the_store_skip_framed_files_being_written, rewrites_take_no_segment_past_a_segment_and_an_extent: 4,800 each, 0 failed.
- Linux, Docker rust:1, same load: 4,800 each, 0 failed; extent_store's store, lineage, views and crash tests, 144, each in a process of its own: 0 failed.
- macOS: cargo nextest -p extent_store 226 of 226; FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast 2,105 of 2,105; clippy clean for the files changed; scripts/upstream-changes.py --check passes. GCE not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Not TASK-184 (fxdax only, outside extent_store's dependency tree) nor TASK-165.5 (extent_remote's WAL, which extent_store doesn't reach): the extent crates are identical on main and land-i. The test's helper segment_data listed segments/ and frame-parsed a framed file that a framing behind a write had created and not yet written. Reproduced on Linux under load, origin/main 5 of 4,800 and land-i 3 of 4,800, with CI's panic. Store::framing names the segments being framed; segment_data and churn_pinning skip their framed files; failpoint segments::framing. segments_read_beside_the_store_skip_framed_files_being_written: red with the skip out, CI's error; green. 4,800 runs each of the three tests on macOS and Linux under load, 0 failed; macOS workspace 2,105 of 2,105.
<!-- SECTION:FINAL_SUMMARY:END -->
