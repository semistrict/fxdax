---
id: TASK-213
title: >-
  Fxfs: a mount counts a shared extent only as far as its last committed range,
  not as far as an allocation dropped in it reached
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 12:23'
updated_date: '2026-11-06 14:16'
labels:
  - bug
  - fxfs
dependencies: []
priority: medium
type: bug
ordinal: 651000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found looking around TASK-211 (2026-11-06). Since TASK-211, a shared (staging) extent takes the range of an allocation dropped in it while it lives on (ExtentTable::drop_allocation; ExtentState::reach), as the extent is written once and its data may be on the device. Nothing journals a drop, so the allocator tree has no record of that range. A mount sizes the extent from its records (ExtentTable::load: as far as its last range), so a dropped range past the last committed one is counted live and not after a remount. The mount then thinks the device holds less than it does. On a full device the allocator could admit writes the device refuses (the fake device's "the device is full"), seen as EIO rather than ENOSPC. Not seen in a test yet. Possible ways: have the commit or the extent's death record its reach, or have the mount ask the device how far each extent it loads is written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test where a shared extent's last allocation drops, the filesystem remounts, and the device is then filled: writes fail ENOSPC (never EIO), and the allocator never holds less than the device stores
- [x] #2 The mount counts each extent as far as the device holds it, or the design records why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A test that fails on main: a shared allocation commits, the next in its extent writes and drops, remount; the extent must count what the device holds.
2. Root cause, and whether a path reaches it.
3. Journal what a drop leaves (AllocatorMutation::Reach), recorded at the next sync and at close.
4. Class oracle in the model: Fxfs counts each extent at least as far as the device holds it, on filesystems no cut touched.
5. Suites and 1,000-case hunts, plain and with FXDAX_MODEL_FSCK=1, on macOS and Linux (Docker).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Test that fails on main
allocator.rs test_a_shared_extent_takes_what_was_dropped_in_it_after_a_remount: a shared allocation commits (0..4096 of the staging extent) and is written; the next in the extent is written and its transaction drops; close, open. On main the extent counts 4096 though the device holds 8192 ("left: Some(4096), right: Some(8192)"). It runs twice, replayed and compacted before the close, and goes red with recording turned off.

## Cause
A drop writes nothing to the journal. Since TASK-211 the live allocator counts the dropped range in the extent (ExtentTable::drop_allocation), but a mount sizes an extent from the tree's records (ExtentTable::load: as far as its last range), which hold only committed ranges. Data the device holds past the last committed range is uncounted after a remount, until the extent dies, which breaks the promise the metadata reservation makes on a full device. The EIO-not-ENOSPC wording in the task is the fake device's: in the guest a full store's write is ENOSPC (fxfs-linux device.rs). The real harm is a compaction or journal write the reservation promised room for being refused.

## Does a path reach it?
Through the allocator, yes (the test above). Through the VFS, rarely: writes reserve their room before they allocate, so a shared allocation drops after its data is written only if something fails between the write and the commit (a device request, say). With recording turned off, a 1,000-case hunt (2,096 cuts) and 400 targeted TIGHT fill-then-remount sequences never reached it. The oracle below found the crash case instead, at the first cut of a pinned sequence, which is TASK-214.

## Fix
AllocatorMutation::Reach { device_range, owner_object_id } (transaction.rs), added last to V60 without a version bump, as Reference and Snapshot were. Allocator::drop_mutation queues a range its extent keeps (Dropped.kept > 0) in Inner::unrecorded_reaches. Allocator::record_reaches commits queued ranges whose extents are alive, in transactions of their own (skip_journal_checks, borrow_metadata_space, at most 256 a transaction). FxFilesystem::sync calls it first (boxed, as an allocation short of room syncs) and so does close, before their barriers; a transaction that drops gives its ranges back to the queue. Applying it (ExtentTable::reach) grows the extent's size and records the range counted 0, which a mount sizes the extent by; an extent no longer alive takes nothing. validate_mutation checks it. fsck accepts the count-0 records: run on the test, it reported only the test's fake owner 99.

## Blind spot and tests (AGENTS.md)
- Why green: nothing compared what the device holds of an extent with what Fxfs counts.
- Class oracle: model_tests.rs World::counts_what_its_device_holds, run by check_all on every filesystem no power cut touched (World::cut; Op::Crash snapshots after a sync, so it counts as untouched). Every extent the device holds must be counted at least as far as the device holds it: the open staging extent as far as it may reach, dead extents not yet freed at their size (Allocator::extent_counted, ExtentTable::counted). Uses a new test-only Device::extent_len (FakeDevice). Two mistakes in my first version, both caught by the hunts: looping sync and free until nothing was dead hung (a sync with nothing new to write doesn't flush, so frees waiting for a recorded flush never come), and checking before frees finished flagged dead extents. Hence counting dead extents at their size.
- Looked around: the same oracle on cut filesystems fails at once, which is TASK-214 (data a cut kept of commits it lost). No record can fix that, so it needs a device request for extents' lengths, which OPEN_QUESTIONS.md asks the user to approve. Until then the oracle skips cut filesystems.
- Structure: the drop path is now recorded where the allocator counts it. What remains is that the device, not the journal, knows how far an extent is written, which only the device can say after a cut.

## Acceptance
- AC 1: the test checks how far the allocator counts the extent against what the device holds, not by filling the device. The fill version passed on main: reserved metadata space, never written, hides a few KiB of undercount from the device's capacity. What it can't hide is the reservation promise, which the count checks directly.
- AC 2: the drop path is counted; the crash path is recorded as why not, in TASK-214 and the open question in OPEN_QUESTIONS.md.

## Verification (on origin/main 76d2c39cf)
- macOS: cargo nextest run -p fxfs -p fxfs-posix -p storage_device 900 of 900; whatever_a_vfs_asks FXDAX_MODEL_CASES=1000: passed (2,040 cuts, 3,619 of replays, 428 s); with FXDAX_MODEL_FSCK=1: passed (2,133 cuts, 3,806 of replays, 749 s). Both with the new oracle running on every uncut filesystem.
- Linux (Docker arm64): the same suite 900 of 900; hunt passed (2,097 cuts, 3,753 of replays, 388 s); fsck hunt passed (2,064 cuts, 3,632 of replays, 638 s).
- cargo check --workspace and the tests of fxfs, fxfs-posix, storage_device, fxdax, follower_sim, fxfs-sim-diff and extent_store compile. upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A shared allocation that drops after writing its data leaves its range in the staging extent. Nothing journaled that, so after a remount the extent counted only as far as its committed ranges, short of what the device held. Now the next sync, or close, journals the range (AllocatorMutation::Reach, added last to V60), which a mount sizes the extent from. A model oracle checks that Fxfs counts each extent at least as far as the device holds it, on every filesystem no power cut touched. Through the VFS the drop path is rare, since writes reserve room before allocating; the allocator test reaches it deterministically, red before and after with recording off. The oracle found the wider case: after a power cut, data the cut kept whose commits it lost (TASK-214). No record can fix that; it needs a device request for extents' lengths, which OPEN_QUESTIONS.md asks the user to approve. Verified: suites and 1,000-case hunts, with and without FXDAX_MODEL_FSCK=1, on macOS and Linux (Docker). GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
