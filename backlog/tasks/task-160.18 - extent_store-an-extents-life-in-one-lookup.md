---
id: TASK-160.18
title: 'extent_store: an extent''s life in one lookup'
status: Done
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-07 13:02'
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
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Prototyped and measured, 2026-11-07, on main 00e541868 (after TASK-165.9; Sealed is views::Sealed, Spent is State::spent). tests/hot_path.rs (ignored; cargo nextest run --release -p extent_store --test hot_path --run-ignored only --no-capture) times, with 20,000 extents held, 20,000 kept and their numbers spent, macOS release, best of 5 rounds, three runs each:
- before: locate 129-136 ns, locate_view 150-154, begin_write refused (sealed or spent) 42-43, free (open) 118-125, write (open, write, finish) 9.0-9.4 us.
- with Life (98b83cc61): locate 121-124, locate_view 143, refused 46-47, free 120-122, write 9.4-10.1 us. Neutral, within noise.
Decided: State::life(extent) -> Life { Open(Option<&Extent>), Held(&Extent), Kept(&Kept), Spent, Unknown } is the one lookup; entry_for, check_opens, begin_write_locked's length and overwrite and Store::extent decide by it, so the order of the probes and what each state means live in one place. Not a single map of every extent's life: it would save at most the two or three hash probes left (tens of ns, on calls a page-in's read and unpack dwarf), and cost restructuring what iterates the held extents (footers, the audit, holds), the kept by the epoch that freed them (KeptExtents' indices) and the open by opening (Uncommitted). Transitions stay where they run, under the state's lock (Store::free, the write's finish, the commit's seal); their cases (TASK-79.1, 79.5, 79.6, 150.6, 150.7) keep their tests. extent_store 158/158.

Verified (landed as land-xk, 4bf0e45fd): macOS full suite (scripts/test.sh: clippy, workspace 2086/2086, sim, every Shuttle part, scripts; again on main 0879c8272: workspace 2079/2079, Shuttle 36/36 twice, sim 48/48, crash rounds 0 of 5 failed); CI 42363963416 green (Linux workspace, sim, Shuttle, scripts, guest); GCE r894 on land-x1: workspace 2305/2305, sim 48/48, Shuttle 36/36, default guest suite 15/16, the one failure extent-guest's next_sequence expectation (33 since TASK-165.9's one recovery, not a lost write), fixed in 101eeb943; GCE r895 on main plus that fix: extent-guest 2/2 rounds, fxfs-guest-soak 2/2 rounds (round 2 with 250 ms latency on its file bucket and the daemon killed). scripts/upstream-changes.py --check passes on main 0879c8272. No upstream file changed by the batch beyond what upstream-changes.py records.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Prototyped and measured: an extent's life is one lookup, State::life, that readers and writers decide by; the hot paths time the same (tests/hot_path.rs: locate ~122 ns, locate_view 143, refused write 46, write 9-10 us), so it stays. A single merged map was decided against: tens of ns at most, for restructuring every iteration of the maps.
<!-- SECTION:FINAL_SUMMARY:END -->
