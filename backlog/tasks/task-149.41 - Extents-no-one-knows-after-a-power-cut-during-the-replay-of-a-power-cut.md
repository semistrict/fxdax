---
id: TASK-149.41
title: Extents no one knows after a power cut during the replay of a power cut
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 08:28'
updated_date: '2026-11-08 03:29'
labels:
  - testing
  - fxfs
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 664000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fxfs-posix model test checks that a filesystem replayed after a power cut holds no extent its allocator does not know (`World::holds_no_orphans`, after syncing until nothing waits to be freed), but checks the replays of cuts made during that replay (`second_cuts` in `Past::check_cut`) only with `check_all`, which counts nothing the device holds. Held to the same check, those replays hold extents Fxfs does not know: found 2026-11-07 while TASK-149.36 made the second replays sync first (the graveyard trims what a truncate a cut stopped short left, the file counting its blocks until then). Reproduction: `an_allocation_over_written_pages_changes_nothing` (Setting::awkward(0, ROOMY).with_graveyard_of_seed(), [Symlink(1), Create(0), FillWhileCompacting{file 0, page 0, pages 13, byte 1}, Allocate{file 0, at 0, len 49153}, Truncate{file 0, size 1}]) with `again.lets_go()` replaced by `again.holds_no_orphans()`: "cut 0, during op 5: cut during its replay: 2 extents unknown". Room leaked so stays taken until something frees it; doc-4 and TASK-213/214 say what a mount after a cut may not count.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The replays of cuts during a replay are held to holds_no_orphans as first replays are, and pass hunts with FXDAX_MODEL_CASES
- [x] #2 Whatever holds the extents is fixed, or the check says why a replay may leave them, with a test red before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Hold the second replays to holds_no_orphans; reproduce (an_allocation_over_written_pages_changes_nothing: 2 extents unknown).
2. Root-cause: allocator.rs on_replay_complete seeks orphans in the 2^16 numbers below the device's next; a mount that found orphans of the range before its floor left their frees to flush_for_frees (FreeAfter::RecordedFlush), after writing past its floor (renew/record_journal_tail); a cut there left them where no later window looks.
3. Fix: orphans nothing replay discarded names die FreeAfter::Now, are freed and sealed (will_flush_device, flush, sealed) before the mount writes; orphans discarded names die wherever they are; a second window past what the tree holds.
4. The residual (discarded-named orphans of a range between two others, cut past the recorded tail) counted apart and filed as TASK-149.41.1.
5. Tests red before; hunts at FXDAX_MODEL_CASES=1200.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause (crates/fxfs/src/object_store/allocator.rs, Allocator::on_replay_complete): orphans were sought only among the 2^16 sequence numbers below the device's next. Each mount mints from its floor, another writer's range 2^40 or more past the last, once a lease moves. A mount that found orphans of the range before queued them FreeAfter::RecordedFlush(0) (extent_table.rs kill_orphans), and Journal::flush_for_frees frees them only after renew_journal_tail and record_journal_tail have written past the floor (filesystem.rs, the mount's sequence). A cut in between left them on the device, with every later window past them: room no one counts.
Fix:
(1) Orphans that nothing replay discarded names die FreeAfter::Now. The mount frees them, then seals the frees (will_flush_device, device flush, sealed) before it writes anything. Invariant: a range holds unfreed orphans only while it's the device's latest. Sealed, a compaction doesn't record them dying (TASK-149.3).
(2) kill_orphans also kills unknown extents that discarded names, wherever they are: a replay of the same journal finds them exactly.
(3) A window past what the tree holds (orphan_windows), for discarded-named orphans left after the recorded tail.
Residual, filed as TASK-149.41.1: discarded-named orphans of a range between two others, after a cut past the recorded tail. They can't be freed before that record without replay's verify failing on an extent it can't read (NoExtent, the TASK-210 failure). The model test counts them apart (holds_no_orphans_but_unsought, by Allocator::orphans_sought) and fails on any orphan inside a window. Pinned tests assert none (Setting::holds), except the one that shows the residual.
Tests (red with the fix out, green with it): allocator test_orphans_are_freed_and_sealed_before_a_mount_writes and test_orphan_windows; extent_table orphans_die_where_sought_and_where_what_replay_discarded_names_them; model tests an_allocation_over_written_pages_changes_nothing (second replays now held to the check), orphans_a_cut_replay_found_are_freed_across_writers_ranges, orphans_a_discard_names_are_freed_after_a_cut_past_the_super_block, orphans_a_cut_replay_found_are_freed_on_a_device_that_fails, and orphans_named_by_a_discard_between_two_ranges_stay_unsought (16, TASK-149.41.1). The last four are hunt seeds.
Hunts (model and corrupt tests, FXDAX_MODEL_CASES=1200, FXDAX_CORRUPT_CASES=1500): before the fix both seeds failed; after, seeds 1-3 and 5-8 pass. Extents left unsought per seed: 8, 8, 13, 0 over about 4,100 second replays each. macOS clippy and workspace: 2115/2115. upstream-changes --check passes.
Blind spot: the second replays were checked with check_all, which counts nothing the device holds. The allocator's floor test covered only orphans below the device's next.
Look around: no other caller of kill_orphans or MAX_ORPHANS_SOUGHT. dead-ends/theory-orphans-after-cuts-on-an-awkward-device.md had a 'Reopen if' that came about; updated.
Structure: orphans are found by searching ranges of numbers, a heuristic over sequence numbers whose writer ranges jump. A device listing of the extents it holds in a range would make the search exact; that, or recording orphans durably, is what TASK-149.41.1 weighs.

Landed in main 8799cb6c4 as land-c8, stacked on land-w7 with land-h9's code (b54c9f125, the fix). CI 42440067129 on 76f18456d green: macOS and Linux workspace among its jobs. land-r3's CI 42437043328 had passed before. The user decided TASK-149.41.1 won't be fixed; the model test's count-apart stays.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A mount frees the orphans it found, and seals their frees, before it writes anything. Before, they waited for the recorded flush, after the mount had written past its floor (another writer's range), and a cut there left them where no later mount looks. Orphans that replay's discarded transactions name die wherever they are, and a second window past what the tree holds finds them after the recorded tail. Replays of cuts during replays are held to holds_no_orphans. Verified by allocator, extent-table and model tests red without the fix (four of them hunt seeds), hunts at 1,200 cases (seeds 1-3, 5-8 pass), macOS 2115/2115 and CI 42440067129. The residual, discarded-named orphans of a range between two others, is TASK-149.41.1, closed won't fix by the user; the model test counts it apart.
<!-- SECTION:FINAL_SUMMARY:END -->
