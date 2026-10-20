---
id: TASK-12
title: Bound host VMAs used by DAX windows
status: Done
assignee: []
created_date: '2026-09-28 19:56'
updated_date: '2026-10-20 15:26'
labels: []
milestone: m-1
dependencies: []
priority: high
ordinal: 12000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The host MMU maps each kernel page with its own mmap, and the Linux uffd second stage duplicates each mapped page again, so a fully populated window (512 MiB = 131072 pages of 4 KiB) needs more VMAs than vm.max_map_count (65530) allows, and mmap then fails with a panic. Coalesce contiguous physical runs or map windows differently so large windows stay within the limit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test populates a window larger than vm.max_map_count pages on Linux without failing
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 (commit 85188c2): the second stage keeps a budget of runs installed, (vm.max_map_count - 4096) / 2, leaving out those used longest ago past it and installing them again on a fault. Test windows_keep_their_mappings_within_the_budget shows it with a budget of 8 over 64 pages; AC1's window larger than vm.max_map_count pages (a 256 MiB+ window of scattered pages) is not yet a test.

AC1 (commit 80f4778): dax::uffd::tests::windows_larger_than_the_map_count_read_whole maps a file of max_map_count + 4096 scattered pages (a run each) into a window with the default budget and reads every page twice: each correct, runs installed never above the budget. Passes on GCE (70 s, debug); the full Linux workspace (2238 tests) passed with it. kernel-cfg.py (DoD2) does not exist in the repo: left unchecked. Mapping the file in 70k pieces made teardown quadratic: TASK-53.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Windows keep their host mappings within a budget derived from vm.max_map_count; a test reads a window of more scattered pages than max_map_count allows mappings, twice, within the budget (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
