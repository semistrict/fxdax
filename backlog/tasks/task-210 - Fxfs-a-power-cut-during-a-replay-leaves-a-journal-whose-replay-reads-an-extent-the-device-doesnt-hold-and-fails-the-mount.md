---
id: TASK-210
title: >-
  Fxfs: a power cut during a replay leaves a journal whose replay reads an
  extent the device doesn't hold and fails the mount
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 10:40'
updated_date: '2026-11-06 12:05'
labels:
  - bug
  - journal
dependencies: []
priority: high
type: bug
ordinal: 649000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42196106048 (land-v, Linux x86, workspace; land-v changes DAX and uffd only) failed fxfs-posix model_tests::whatever_a_vfs_asks_reads_back_as_a_model_says after 305 s: "cut 0, during op 20: cut during its replay: opening: Journal replay failed: Failed to validate checksums: no extent ExtentId { branch: 1, sequence: 507722380628 }". Reproduces on main (1fefc017c, macOS) deterministically from the input proptest printed (seed 8160502985120782900, ROOMY). A power cut during a replay left a store whose next replay reads a data extent the device does not hold, in checksum verification (checksum_list.rs verify), and NoExtent fails the mount rather than ending the journal. A 1,000-case hunt that morning found nothing: rare.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The sequence pinned as a model test, failing before the fix
- [x] #2 Its cause named with file:line: why the journal after a cut during replay names an extent the device does not hold
- [x] #3 Fixed at the root; the pinned test passes
- [x] #4 Model hunts of 1,000 cases or more pass, with and without FXDAX_MODEL_FSCK=1; fxfs and fxfs-posix suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Pin the CI input as a model test (red).
2. Trace extent 507722380628 through both replays (temporary prints on the fake device, the checksum list and replay).
3. Fix in ExtentTable::settle_replay: every dead extent a mount found that a discarded transaction names waits for a recorded flush, however it died. Unit test and a deterministic every-cut-of-the-mount test, each red with the bug back.
4. Hunts: whatever_a_vfs_asks at 1,000 cases with and without FXDAX_MODEL_FSCK=1, and the fxfs and fxfs-posix suites, on macOS and on Linux (Docker).
Revised in the doing: a coarse rule (free nothing after any discard) was tried and dropped, as a full device needs the unnamed frees to renew the journal tail; cutting every replay in the every-cut test found nothing at ten times the cost. Both are in dead-ends/.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Reproduction
CI attempt 1 printed the input (shrinking aborted at 384 iterations): seed 8160502985120782900, ROOMY, 21 ops ending in FillWhileCompacting { file: 5, page: 42, pages: 10, byte: 189, flushing: true }. Deterministic on main (1fefc017c, macOS), 0.1 s. Pinned: model_tests.rs a_cut_during_a_replay_leaves_a_journal_that_replays, red before, green after. No single deleted op keeps it red, nor did 64 other seeds per deletion (the cuts are seeded per barrier), so the 21 ops stay.

## Cause
Traced extent 507722380628 with temporary prints on the fake device, the checksum list and replay. Cut 0 fell during op 20. The fill had minted the extent and its transaction had not committed when the concurrent forced compaction wrote the allocator info, whose dying list includes the extents minted with nothing committed (extent_table.rs ExtentTable::recorded_dying: `dying.extend(self.uncommitted…)`). The cut kept the journal and lost the fill's data. Replay 1 (sb offset 17899, journal_end 24576) failed the checksum of the transaction at 25857, which wrote the extent, and discarded it (valid_to 25857, discarded_to 28672). journal.rs Journal::extents_verified_by named the extent among those discarded. But ExtentTable::load had queued it from the dying list with FreeAfter::Now, the premise being that no transaction after a compaction allocates in what it recorded dying (true of dead extents, false of minted ones). ExtentTable::settle_replay reconsidered only FreeAfter::Settling, so the replay discarding the allocation left nothing to revive the extent, and Allocator::on_replay_complete's free_dead freed it at once, before the journal recorded the discard. The second cut kept that free, and a later barrier set Root::journal_end to 25857 with no Discard record written yet. Replay 2 read the transaction at 25857 again and verified its write in an extent the device no longer held: NoExtent, which ChecksumList::verify propagates (checksum_list.rs, `device.read(...).await?`), failing the mount.

So this is neither the read-ahead window rule nor a wrong journal_end: the replay read a block the journal still legitimately holds, and the free came too soon. It is the same class as CI run 41876989384 (da6ad5f19: the staging extent emptied, killed in replay): a mount freeing what a discarded transaction may still name.

## Fix
ExtentTable::settle_replay decides for every dead extent the mount found, however it died: if what replay discarded names it, it waits for a recorded flush (RecordedFlush, keeping any offset), which the Discard record precedes; the rest stay as they died, Settling at once. It had decided only for Settling, the deaths replay and load's empty extents make. Death sites no longer need to know about discards: settle_replay is the one place that does.

Tried and given up: freeing nothing after any discard (a bool in place of extents_verified_by), which would need no per-extent naming. The hunt found a full TIGHT device that mounted no more: the mount renews the journal's tail before flush_for_frees, and the reallocation needs the room of the frees made at once (seed 4758196049501471983, passes on main and with the per-extent rule; pinned as a_full_device_mounts_with_the_room_of_what_it_found_dead; dead-ends/mount-frees-nothing-after-any-discard.md).

## Blind spot and tests (AGENTS.md)
- Why green: the three conditions rarely meet. A compaction must run while a fill's extent is minted but uncommitted, a cut must lose that data and keep the journal, and a cut of the remount must come between the free and the discard's record. Only the random hunt cuts replays (1 in 2 barriers); the every-cut mode never does.
- Class tests, each red with the bug put back (Now kept Now when named): extent_table.rs a_mount_settles_what_it_found_dead_by_what_replay_discarded, every kind of death a mount finds (load's dying, empty, owner deleted, replay's, orphan), named and not; write_many.rs test_every_cut_of_a_mount_discarding_a_write_to_an_extent_minted_as_it_compacted, deterministic (a write minted, force_compact, commit, the data corrupted, Crashes::every over the mount), which fails "cut 5 of the mount: … no extent", the CI error; and the pinned model test.
- Tried and given up: cutting every replay at every barrier in every_cut_of_a_short_sequence. With the bug back, 300 cases (20,186 cuts, 82,904 replay cuts, 1,489 s) passed, at about ten times the cost per case (dead-ends/every-cut-model-test-cutting-replays.md).
- Looked around: every FreeAfter a mount assigns now goes through settle_replay. Live deaths are Flush(offset) past the Discard, which precedes every later transaction. Structure: what still rests on per-extent precision is Journal::extents_verified_by naming everything a later replay verifies (written ranges, Allocate, Deallocate and Reference). A full device needs the unnamed frees, so the coarse rule can't replace it.
- Found by the hunts, not this change: TASK-211, a clean remount of a full TIGHT device that replay's object manager can't reserve metadata for (fails on 36cd32147 with and without this fix).

## Verification (final code, on origin/main 36cd32147)
- macOS: cargo nextest run -p fxfs -p fxfs-posix 846 of 846; whatever_a_vfs_asks FXDAX_MODEL_CASES=1000: passed (2,024 cuts, 3,582 of replays, 469 s); with FXDAX_MODEL_FSCK=1: passed (2,158 cuts, 3,875 of replays, 869 s).
- Linux (Docker arm64, rust:1): the same suite 846 of 846; hunt 1,000 cases passed (2,061 cuts, 3,622 of replays, 367 s); fsck hunt 1,000 cases passed (2,095 cuts, 3,673 of replays, 764 s).
- scripts/upstream-changes.py --check passes. Workspace suite and GCE not run (DoD #1 open); the change is in fxfs and fxfs-posix only.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A mount freed an extent that a compaction had recorded dying while minted with nothing committed: a transaction after it had allocated in it, which replay discarded, and a cut before the journal recorded the discard left a store whose next replay verified that write in an extent freed (NoExtent from ChecksumList::verify). ExtentTable::settle_replay now holds every death a mount finds until a recorded flush if what replay discarded names it, however the extent died. Tests, each red with the bug back: a deterministic every-cut-of-the-mount test (write_many.rs), a settle test of every kind of death (extent_table.rs), and the CI sequence pinned (model_tests.rs); plus a full-device sequence that caught the coarser rule tried first. Verified: fxfs and fxfs-posix suites and 1,000-case hunts, with and without FXDAX_MODEL_FSCK=1, on macOS and Linux (Docker), all green. GCE not run. Found by the hunt: TASK-211.
<!-- SECTION:FINAL_SUMMARY:END -->
