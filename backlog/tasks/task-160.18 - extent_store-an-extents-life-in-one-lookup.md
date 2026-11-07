---
id: TASK-160.18
title: 'extent_store: an extent''s life in one lookup'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-07 09:33'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 589000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An extent's state is spread over State.extents, Uncommitted.open, KeptExtents, Spent and Sealed; each reader works it out itself (entry_for, check_opens, the write finish, free's branching). Explains TASK-79.1, 79.5, 79.6, 150.6, 150.7. Proposal: one Life lookup and transitions as State methods. Hot path: prototype and measure first.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Prototyped and measured
- [x] #2 Decided
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Prototyped and measured, 2026-11-07, on main 00e541868 (after TASK-165.9; Sealed is views::Sealed, Spent is State::spent). tests/hot_path.rs (ignored; cargo nextest run --release -p extent_store --test hot_path --run-ignored only --no-capture) times, with 20,000 extents held, 20,000 kept and their numbers spent, macOS release, best of 5 rounds, three runs each:
- before: locate 129-136 ns, locate_view 150-154, begin_write refused (sealed or spent) 42-43, free (open) 118-125, write (open, write, finish) 9.0-9.4 us.
- with Life (98b83cc61): locate 121-124, locate_view 143, refused 46-47, free 120-122, write 9.4-10.1 us. Neutral, within noise.
Decided: State::life(extent) -> Life { Open(Option<&Extent>), Held(&Extent), Kept(&Kept), Spent, Unknown } is the one lookup; entry_for, check_opens, begin_write_locked's length and overwrite and Store::extent decide by it, so the order of the probes and what each state means live in one place. Not a single map of every extent's life: it would save at most the two or three hash probes left (tens of ns, on calls a page-in's read and unpack dwarf), and cost restructuring what iterates the held extents (footers, the audit, holds), the kept by the epoch that freed them (KeptExtents' indices) and the open by opening (Uncommitted). Transitions stay where they run, under the state's lock (Store::free, the write's finish, the commit's seal); their cases (TASK-79.1, 79.5, 79.6, 150.6, 150.7) keep their tests. extent_store 158/158.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Prototyped and measured: an extent's life is one lookup, State::life, that readers and writers decide by; the hot paths time the same (tests/hot_path.rs: locate ~122 ns, locate_view 143, refused write 46, write 9-10 us), so it stays. A single merged map was decided against: tens of ns at most, for restructuring every iteration of the maps.
<!-- SECTION:FINAL_SUMMARY:END -->
