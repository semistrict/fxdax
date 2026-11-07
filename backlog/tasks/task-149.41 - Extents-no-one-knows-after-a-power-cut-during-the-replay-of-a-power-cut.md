---
id: TASK-149.41
title: Extents no one knows after a power cut during the replay of a power cut
status: To Do
assignee: []
created_date: '2026-11-07 08:28'
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
- [ ] #1 The replays of cuts during a replay are held to holds_no_orphans as first replays are, and pass hunts with FXDAX_MODEL_CASES
- [ ] #2 Whatever holds the extents is fixed, or the check says why a replay may leave them, with a test red before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
