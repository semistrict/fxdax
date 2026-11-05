---
id: TASK-171
title: >-
  fxfs-guest-checks-dax: stores_under_rebinds rebinds only by chance, none in
  r582 and r583
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:05'
updated_date: '2026-11-05 08:29'
labels:
  - bug
  - dax
  - test
dependencies: []
priority: medium
ordinal: 634000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
stores_under_rebinds asserts that some page was rebound in its two seconds. Its loop stored to all 64 pages in turn, each every 64 rounds, so a page stays clean from its writeback through the seal to its rebind only if the loop stalls; past GCE runs rebound 3,131 pages at 51,504 rounds and 31-45 at about two million. r582 (2,013,185 rounds) and r583 checks-dax (1,493,249) rebound none, while r583's -small and -delay (about 1.39M) rebound 178 and 103, and r591/r592 86 and 174: by chance, not a regression.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each page of the check goes unstored for many of the worker's rounds, whatever the loop's speed, so it is rebound; stores still race the rebinds
- [x] #2 checks-dax passes on GCE, jailed and not, with pages rebound in the hundreds or more
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Store to a hot set of 8 of the 64 pages that moves on by a page every 2 ms: each page unstored for ~112 ms, eleven of the worker's 10 ms rounds. 2. GCE checks-dax, jailed and not.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Data (stores_under_rebinds' own line, every GCE run that has it): rebinds fall as rounds rise. 51,504 rounds gave 3,131 rebound; 1.0-1.4M gave 31-1,424; about 2M gave 31 and 45. Then 0 in r582 (2,013,185 rounds) and r583 checks-dax (1,493,249), while r583's -small and -delay rebound 178 and 103 at about 1.39M and r591/r592 86 and 174. Not a regression: r591, on the old base, passed.
Cause: the loop stored every page in turn, each every 64 rounds. A page is rebound only if it stays clean from its writeback through the seal (fxfs_fsync) to the rebind's down_write, so only when the loop stalls in that window. Whether it does depends on timing.
Blind spot: the check relied on stalls it didn't make. Its 'some page rebound' guard caught the gap, but intermittently.
Fix: stores go to a hot set of 8 pages that moves on by a page every 2 ms. Each page goes unstored about 112 ms, eleven of the worker's rounds, whatever the loop's speed. Stores still race the rebinds of the pages beside the hot set, and each page's own as the set comes back to it.
Looked around: rebind_against_loads_and_stores uses pause points, deterministic. No other check asserts a rebind count over a free-running loop (rg dax_rebound_pages in test_fxfs_guest_checks.rs).

Verified: stores_under_rebinds rebound 995 pages at 1,243,078 rounds (r593, jailed) and 991 at 1,558,974 (r594), against 0-45 near those speeds before. Both checks-dax runs passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
stores_under_rebinds stored to all 64 pages in turn, so a page was rebound only if the loop happened to stall between its writeback and its rebind. Fast hosts rebound none (r582, r583). It now stores to a hot set of 8 pages that moves on by a page every 2 ms, so each page goes unstored for about 11 worker rounds. GCE r593 (jailed) and r594 rebound 995 and 991 pages, and checks-dax passed both times. Not a regression: r591 on the old base passed, and so did r592 on main.
<!-- SECTION:FINAL_SUMMARY:END -->
