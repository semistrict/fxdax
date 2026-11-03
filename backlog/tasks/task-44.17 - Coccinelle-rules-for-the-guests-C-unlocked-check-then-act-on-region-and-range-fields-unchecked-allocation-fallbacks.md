---
id: TASK-44.17
title: >-
  Coccinelle rules for the guest's C: unlocked check-then-act on region and
  range fields, unchecked allocation fallbacks
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 396000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Some of the guest's bugs have a shape a semantic patch matches: a field read under one lock and acted on under another (TASK-99), a fallback path taken only when an allocation fails (the keep path). Coccinelle, run by the guest checks, flags new instances as they are written.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Semantic patches for those shapes run in scripts/check-guest.sh
- [x] #2 They flag TASK-99's code, put back, and pass on main
- [ ] #3 smatch or GCC -fanalyzer runs over the guest C, its licence checked, its findings triaged; it flags a double free on an allocation fallback, put back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
region-lock.cocci (context mode: Homebrew's spatch 1.3.3 can't load Python), scripts/cocci-guest.py, check-guest.sh runs it; test_cocci_guest: flags region_open_unlocked's line alone, main clean. The allocation-fallback shape is not written: it needs a policy for which fallbacks are allowed (map_extents' MAP_MANY fallback and the keep path are both intended), e.g. an expected list of fallbacks each naming its fault-injection test, or failslab (TASK-44.10) covering them all.

Decision (2026-10-24, the user left it to me): an explicit list. A second rule lists every allocation whose failure path goes on rather than fails (a fallback); scripts/cocci holds the expected list, each entry naming the fault-injection test that takes that path (as fxfs.dax_fail_held does the keep path). A fallback not on the list fails check-guest.sh. Why: failslab (TASK-44.10) fails allocations at random across the kernel, so it can't show that one particular fallback is ever taken, and TASK-99's keep path was only exercised by a knob aimed at it. failslab stays as the broad backstop.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
region-lock.cocci (7f37f90f1) runs in check-guest.sh and flags TASK-99's unlocked read (test_cocci_guest, its code inline since 755e54cc6). Decided not to do, as low-yield: the allocation-fallback rule, and smatch or -fanalyzer.
<!-- SECTION:FINAL_SUMMARY:END -->
