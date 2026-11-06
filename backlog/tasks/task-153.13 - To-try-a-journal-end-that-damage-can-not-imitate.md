---
id: TASK-153.13
title: 'To try: a journal end that damage can not imitate'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 23:09'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 560000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: a bad block reads as the end of the journal, so a flip mid-journal silently loses synced transactions: 25 to 32 of 200 cases on a crashed image. Needs a durable end marker outside the journal.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Designed, with what it costs a commit
- [x] #2 Decided
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design: the store's root, which a barrier sets atomically with everything before it (doc-4), carries journal_end: how far the journal had been written before the barrier that set it (block-aligned). A sync's barrier sets it: the journal flush's POST_BARRIER write (WriteOptions.root, one request: WRITE_COMMIT gains WRITE_ROOT on the wire), or flush_device's commit (commit_root in place of flush), or a super-block's. Not while the tail is unrecorded (renew_tail), as a replay from the old super-block can't read it. One async lock over the root: barriers set roots in the order they compose them, so a super-block's root never goes back. Replay refuses (Inconsistent) when the end of what it replays (chain break, bad mutation, data checksum) is before journal_end; past it, a torn tail is a torn tail. Offset only: the chain from the super-block's checkpoint proves the bytes are that journal's.
Cost a commit pays: no request more anywhere; the root's ~40 bytes more in the sync's request and in the host's commit record.
1. Refactor: WriteOptions threads through Fxfs's writes in place of WriteFlags.
2. storage_device: WriteOptions.root; fake, image, follower_sim devices set it with a POST_BARRIER.
3. Wire: WRITE_ROOT (with WRITE_COMMIT) in virtio_extent.h, driver.c, fs/fxfs device.c/h, Firecracker, extentd wire/client/server, ExtentStore::write_with_barriers; guest device.rs; doc-4.
4. Fxfs: Root.journal_end, the root lock, the journal's barriers carrying it, replay's check.
5. Tests: a flip mid-journal below the end is refused, a torn tail past it mounts (journal.rs); corrupt_tests: no unsealed journal flip reads back different; update a_flip_in_data_being_written (now refused).
6. cargo nextest workspace on macOS, GCE run incl. guest kernel.

Revised in the doing: the end recorded is the journal's valid_to, not its written end: after a replay that ended a torn tail, the journal goes on at the next block, and a flush before anything is written there recorded that block, which a replay after a second crash doesn't reach (found by four model_tests).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Design and decision (2026-11-03)

Lead checked: the device's root is set atomically with a barrier on every device (fake: barrier then root; image and follower_sim: store commit with the root; guest: COMMIT_ROOT), and only the super-block's save wrote it. A sync's barrier is a COMMIT (flush_device) or, on the guest's fsync path, the journal write's WRITE_COMMIT (POST_BARRIER), which had no room for a root.

Decided yes: Root gains journal_end, how far the journal was valid when a barrier set it. Replay refuses (Inconsistent) when it ends before it; past it, a torn tail is a torn tail. Offset only: the chain of block checksums from the super-block's checkpoint already proves the bytes up to it are that journal's, so a checksum adds nothing.
- Who sets it: the journal flush's POST_BARRIER write (WriteOptions::root, OverwriteOptions::root), and flush_device's flush, now commit_root. Not while tail_unrecorded (renew_tail): a replay from the last super-block can't read that tail. Super-block saves carry it unchanged.
- One owner: SuperBlockManager::set_root holds an async lock over composing and setting the root, so a root composed before another is never set after it (that would name a freed super-block). The device's seal serialized barriers already.
- The wire: WRITE_ROOT (bit 3, needs WRITE_COMMIT), root_len in the io args' old reserved field, the root after the data: virtio_extent.h, driver.c (+selftest), fs/fxfs device.c/h, guest device.rs, Firecracker (a7a5791e2 on semistrict/firecracker fxdax-v1.16.1), fxdax ExtentStore::write_with_barriers and extentd's wire (WRITE_ROOT, the root after the data, args[4] the data's length), doc-4.

**Cost a commit pays:** no request more on any device. A sync's barrier carries the root (~50 bytes: magic, two extent ranges, the end), copied into the request and kept in the host's commit record (Delta.root, which a COMMIT_ROOT already filled). Barriers that set roots take turns in Fxfs, as they did at the device. Rejected: an extra COMMIT per fsync (a round trip each, which WRITE_COMMIT exists to save); a marker only on durable syncs (sync/syncfs), leaving fsyncs unprotected; nothing, trusting the host's CRCs (TASK-153.11), which ordering bugs and host memory get past.

**Behavior change:** a synced write whose data the journal's checksums find damaged now refuses the mount (a_flip_in_data_being_written_is_found_by_journaled_checksums), where replay rolled the file back silently.

## Evidence
- journal.rs test_a_journal_ending_before_its_synced_end_is_refused: a flip in the last block a sync wrote is refused; one in the first block after mounts, synced files there, unsynced gone, fsck clean. Red with the check out (mounted).
- corrupt_tests: unsealed journal flips never read back different. 400 cases of the crashed journaled image, same seeds: without the check 5 of 20 different, 1 refused; with it 0 different, 6 refused. 4,000 cases of the crashed barriers image: 200 unsealed journal flips, 32 refused, 0 different (TASK-149.9 saw 25-32 different there); closed barriers image, 4,000 cases: 0 different. The 4,000-case crashed journaled run hit the 30-min limit unfinished.
- extentd test a_writes_commit_sets_its_root, Firecracker worker test_roots_and_info (WRITE_ROOT, and EINVAL without WRITE_COMMIT), fake device test_post_barrier_sets_the_root, driver selftest.
- macOS scripts/test.sh: 2,012 of 2,012, Shuttle suites, scripts' 135. fxfs-linux checks for aarch64-unknown-none-softfloat.

## Blind spots (AGENTS.md)
- My own bug in the doing: flush_device recorded flushed_offset, which after a replay ending mid-block is a block replay doesn't reach until something is written there. Four model_tests found it (crash, remount, crash again): the class test exists and worked. The journal's own test didn't remount; the model tests do, at random cuts.
- test_generation_wrapping_on_flush saved super-blocks through a second SuperBlockManager on a live filesystem, whose barriers then set the root from their own: a test artifact (one owner of the root is the point), fixed to use the filesystem's.
- The 4,000-case run found a pre-existing panic, a sealed super-block flip at reader.rs:159 (seed 741, reproduces on origin/main): filed TASK-149.23.
- Structure: the class (damage reads as an end) is now impossible for what a barrier covered; what remains is the window while a renewed tail is unrecorded, and sealed (resealed) flips, which no checksum can find.

## Verification at the final commits (rebased on origin/main 457d500b0)
- macOS scripts/test.sh at f6df179e5's rebase: 2,043 of 2,043, Shuttle suites, scripts' 135; after the rebase on 457d500b0, fxfs, fxfs-posix, storage_device and fxdax: 1,153 of 1,153. upstream-changes.py --check passes.
- GCE fxdax-test-t153j-journal-end4 (guest kernel built from the source): workspace 2,208 of 2,208 and the Shuttle suites; Firecracker's vmm extent tests 14 of 14; guest cases 13 of 14, every crash case (crash, crash-dax, crash-dax-killed, crash-stray-dax) and checks, pgbench, docker among them. extent-guest failed: the driver selftest's new WRITE_ROOT step left its own root, which the test reads (the test working). The selftest puts its root back now, the test counts its extent; GCE fxdax-test-t153j-journal-end5: extent-guest passes. Every t153j host deleted.
- Linux-only code (extentd, Firecracker) type-checked from macOS with zig cc for x86_64-linux-gnu after the first GCE run found two compile errors there.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**The journal's end is in the root, set by each sync's barrier (TASK-153.13)**

A flip mid-journal read as its end, losing synced transactions unsaid. Decided: the root records how far the journal was written when a barrier set it (`Root::journal_end`), and a replay ending before there is refused as corrupt; past it, a torn tail is still a torn tail. A sync's barrier sets it: its journal write's `WRITE_COMMIT` carries the root (`WRITE_ROOT`, new on the wire), or its flush becomes a commit of the root. Cost: no request more; the root's ~50 bytes in the sync's request and the host's commit record; barriers setting roots take turns (they did at the device already). A synced write whose data the journal's checksums find damaged now refuses the mount, where it rolled the file back.
**Instead:** a durable marker only on `sync`/`syncfs` (fsyncs unprotected), or an extra `COMMIT` per fsync (a round trip each), or no marker, leaving it to the host's CRCs, which bugs in ordering get past.
**Undoing it:** drop the replay check; the root's field and `WRITE_ROOT` can stay unused.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Decided yes, and done. The store's root records how far the journal was valid when a sync's barrier set it (Root::journal_end), and replay refuses a journal ending before there (Inconsistent); past it, a torn tail is still taken. Cost a commit pays: no request more. A sync's barrier carries the root, ~50 bytes, in its one request (WRITE_ROOT with WRITE_COMMIT, new on the virtio device, extentd's wire and Firecracker), and the host keeps it in that commit's record. Barriers that set roots take turns in Fxfs (SuperBlockManager::set_root). A synced write whose data the journal's checksums find damaged now refuses the mount, where it was rolled back silently. Verified: a journal test (red without the check), the corrupt-image class assertion (5 of 20 silent losses become refusals on the same seeds; 0 different in 200 unsealed journal flips at 4,000 cases), device and wire tests, macOS and GCE suites with guest crash cases. Found and filed: TASK-149.23, a pre-existing panic on a sealed super-block flip.
<!-- SECTION:FINAL_SUMMARY:END -->
