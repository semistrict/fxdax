---
id: TASK-149.9
title: >-
  Corrupt images: a flipped byte fails a mount, or fsck says so, and nothing
  panics
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-01 23:08'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 510000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest's disk is trusted by nobody after a host failure or a bad upload. Fxfs has cfg(fuzz) Arbitrary derives in 12 files and no fuzz targets. Verify cargo-fuzz's license before adding it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Fuzz targets cover journal records, layer files and the super-block
- [ ] #2 Flipped bytes in an image give an error from mount or fsck, never a panic
- [x] #3 Panics found are fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Class test in crates/fxfs-posix/src/corrupt_tests.rs: real images (journaled checksums or barriers; closed or crashed), mapped into parts, flipped, sealed where checksums stand in the way, mounted, fscked, read and written. 2. Fuzz targets under crates/fxfs/fuzz (journal, layers, super-block) from the Arbitrary derives, in a workspace of their own. 3. Each panic a returned error with a pinned regression; search for the same pattern; record it here.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Results (2026-11-01, branch test/corrupt-images)

**Class test:** crates/fxfs-posix/src/corrupt_tests.rs. Four images (journaled data checksums or barriers; compacted and closed, or crashed with a journal to replay), each mapped into parts (root, super-blocks, journal, store info, root/allocator/volume layers, allocator info, file data, anywhere). Each case changes 1-3 places (bits, bytes, runs, awkward numbers, huge varints, whole blocks, lost or short extents), seals them where checksums would stop it, then mounts read-only (fsck, read everything) and writably (read, write, sync, compact, fsck, unmount). A bounded allocator, a watchdog and a panic hook make a panic, hang or >1 GiB held fail. 120 cases an image by default; FXDAX_CORRUPT_CASES and FXDAX_CORRUPT_SEED to hunt.

**Run:** 100,240 cases in all. Of those, 68,240 had "closed" images that were really crash snapshots (a harness bug, below), so about half of those repeat; 48,000 ran on four distinct images, the last 16,000 on the final code: no panic, hang or over-allocation.

**Fuzz:** crates/fxfs/fuzz, seven targets. cargo-fuzz 0.13.2 is `MIT OR Apache-2.0`. libfuzzer-sys 0.4.13 is `(MIT OR Apache-2.0) AND NCSA`; its README says "All files in the `libfuzzer` directory are licensed NCSA", and those files' own headers say `Apache-2.0 WITH LLVM-exception`. NCSA is OSI-approved (SPDX isOsiApproved true, opensource.org lists it); Apache-2.0 is, and the LLVM exception only adds permission. Five rounds: 38 target-hours, 76 core-hours, about 6.1M executions of image targets and 306M of lsm_tree_actions. Round one had ASan, the rest none, for speed. Final code: one hour of five targets with compaction, no crash, timeout or OOM. The `#[fuzz]` modules never compiled here: Fuchsia's `fuzz` crate isn't in the workspace, and their 512-byte device can't hold Fxfs. They are plain functions now.

## Panics and hangs fixed

Found by a run: a writer dropped with items by a failed compaction (persistent_layer.rs:1554); a layer's trailer length overflowing (:727); tombstoning what a graveyard shouldn't hold (object_store.rs:1458, 1462, with graveyard.rs:351 following it); a key below its block's base in a compaction (serialized_key.rs:141); a live transaction that can't apply (journal.rs:2211); an unaligned allocator range reaching a key (extent.rs:115); a store or root parent with no graveyard (object_store.rs:2713, 857 fuzz crashes, one bug); a bloom filter spinning 2^44 hashes on a long key in a compaction (found as a fuzz timeout: a hang, in a compaction).
Found by reading, step 3 of the blind spot rule, each with a test: the lead, a journal start in a block's seal (with the check out a real mount panics at reader.rs:304, the same class as reader.rs:157), a start before the journal's extents (bootstrap_handle.rs asserts), truncating outside a handle, a record as long as a block (`consume` asserts), journal extents overflowing, ChecksumList dividing by zero or by too few, an unchecked super-block record, extent keys checked only when the extent had a value, a bitmap short of its extent, checksums or bits that don't fit one (`entries_in`), a huge attribute size, block counts wrapping past their check, a layer longer than it says (each chunk is an entry in memory), a count of items sizing a filter, a bloom filter's hashes (a query's length), fsck on a file size near u64::MAX, a tombstone of a non-object.
Each has a pinned regression. All went red with the bug put back, or spun past 25s (the bloom filter).

**Blind spots.** No test ever fed Fxfs anything but what it wrote: the path never ran. Debug assertions on disk state hid in release builds (TASK-149.1's kind). Sealing was needed: an unsealed flip stops at the first checksum. Fuzz targets that don't compact miss what only writing layers asserts, and the class test's random flips rarely make a valid huge key.

**Harness bugs, mine.** `Crashes::last` records the next barrier, the close's first, so "closed" images were crash snapshots: equal counts showed it, and fixing it found extent.rs:115. a_closed_image_has_next_to_nothing_to_replay pins it. The flush task starts at the first commit, so a raw journal write and close hung (not a bug in Fxfs). `Extent`'s derived Arbitrary made keys no reader can.

## Does the structure allow the class? (the rule that bugs question the structure)

Searched `backlog search` and dead-ends/: no repeat. Nearest: TASK-79.56 (a crafted image in the importer, another crate), TASK-139 (the journal's own invariants).
- Backward ranges: refactored. ExtentRange doesn't deserialize backwards (as Extent's `try_from`); ad-hoc checks removed. Its field stays public, so in memory it can still hold one.
- Key below base: refactored. `serialize_key_with_base_into` returns it; a search and a writer no longer each have to know.
- Layer sums: refactored. LayerLayout, the one owner of them, then the checks in it.
- Writer dropped with items: structure fine. The assert couldn't tell an error path from a forgotten complete.
- Journal start, offsets, record length: structure fine. One place reads a checkpoint.
- Super-block records: shared the one check (`validate_mutation`).
- Allocator ranges: one predicate for three mutations. A type would need the block size, which storage_device doesn't know.
- Graveyard ID: boundary checks at the two places that read one; the assert stays, an in-memory invariant.
- Bloom insertion: the reader capped partitions and the writer didn't: now both.
- Tombstones, bitmaps, `entries_in`, and key order: fixed as they stand; see 1 below.
- Live apply: fixed as it stands; see 4 below.

## Follow-up, ranked

Architectural, for the owner's decision:
1. No trust boundary for layer records. Journal and super-block records pass `validate_mutation`; layer records are trusted as they're read, then checked piecemeal, or not. It explains key below base, the short bitmap, `entries_in`, a tombstone of a non-object, out of order keys. Change: check each item once, in `take_item`, with the same predicate as the journal's, which upstream's TODO fxbug.dev/42178223 in extent_record.rs asks for. Costs a check per item on the hottest read path: measure first.
2. Checksums written and never read. Cow extent records hold fletcher sums, on layer files too (seen in an image: stores 3 and 4), which no read or fsck checks. Checking them when layers, store info and allocator info are read would find most metadata flips with no format change. Data: the same at a cost per read.
3. A journal's end and damage to it look alike. A flip mid-journal ends replay quietly, losing synced transactions (25-32 of 200 cases on a crashed image read back different, no error), and so does `validate_mutation`. Needs a durable record of where the journal ended, outside it. Format-adjacent.
4. Apply follows the journal write, and can fail. Checks made at apply can't be made before the write under concurrent transactions, so a failure can only stop the journal. A failed commit now stops it, which turns a panic into a dead filesystem: right for a guest, but every later write fails.

Testing, smaller:
5. Encrypted volumes, followers (`read_on`) and image-builder mounts aren't in the images.
6. `allocate_buffer` panics or waits on a size past its pool: MAX_ATTRIBUTE_READ guards `read_attr`, an interface that can't fail guards nothing else.
7. layer_bytes stays at 23 corpus entries: seed it with real layers. Image targets run about 50 exec/s.
8. graveyard.rs:351 `unwrap`s if the reap task died: only after a panic, so left.

## What each mode proves

Guest mounts use the default, `barriers_enabled` false: data checksums journaled. (The task's description has it the other way round.) Numbers: 16,000 cases, final code, journaled/barriers.
- Super-blocks, both modes: fletcher64 covers them. Any flip in every block read of both is refused (a_flip_in_both_super_blocks_is_refused); one instance's falls back to the other.
- Journal, both modes: fletcher64 chains it, but a bad block reads as the end: no error unless before the super-block's offset.
- Data, barriers false: journaled checksums are checked only at replay, only for writes since the last device flush, and a mismatch ends replay there, as for a lost write (pinned). 9 of 800 refused.
- Data, barriers true: nothing checks it (pinned: reads the flipped byte). 0 of 800 refused.
- Past that flush, in both: reads and fsck check no data. About 84% of FileData cases read back different with no error: silent wrong data, in scope to report only. DAX writes never reach `write_aligned`, so they have no checksums at all (from reading the code, not from a run).
- Layers, allocator, store info, both modes: no checksum read. What's proved is that format checks and fsck don't panic: AllocatorLayer 51% refused, 1% fsck, 48% same; VolumeLayer 14% refused, 31% failed a read, 1% different; StoreInfo 10% refused, 5% fsck.
- Sealed cases show the parsers behind the checksums: the same in both modes.

## Verification

Commit 72907ee3b: macOS `cargo nextest run --workspace` 1952 of 1952. Linux (Docker, not Lima) 2090 of 2091: only tenant_sockets_work_bind_mounted_into_jails, the known Docker failure. Under fuzz's load the_daemon_stops_on_sigterm, hundreds_of_writers_flush_on_threads_they_share and durable_commits_keep_the_open_segments_pages_cached_on_the_host each failed once: alone, and in the final run, they pass. `scripts/upstream-changes.py --check`: passes. `cargo check -p fxfs-linux` for aarch64-unknown-none-softfloat at the final commit: passes. Every commit up to 492de03e9 was compile-checked alone; those after were tested as they were made.

**Not done, or unverified.** AC 2 isn't met as written: a flip never panics, but flips outside checksummed structures give no error (see above). No guest kernel was built or booted, so nothing here ran in a kernel. test_misaligned_allocation changed its setup: a journal's misaligned allocation is now refused by replay, so it inserts into the tree directly, as test_malformed_allocation does. Fuzz ran on arm64 only.
<!-- SECTION:NOTES:END -->
