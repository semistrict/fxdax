---
id: TASK-157
title: >-
  Tests that fail under load: forks failing at each step, and windows sharing a
  base
status: Done
assignee:
  - '@claude'
created_date: '2026-11-02 01:59'
updated_date: '2026-11-05 10:17'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 562000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-150.20 under load: guests_forks_failing_at_each_step_harm_nothing fails 10 of 10 on base and 8 of 10 on head; windows_share_bases_pages 136 of 500 on base, 41 of 500 on head. Neither is that work. Each is a race in the test or in the code: not known which.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each cause found, in the test or the code
- [x] #2 Each passes 500 runs under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce each under load on 9f81f6018 (Docker, stress, parallel runs). 2. Trace each race to its cause. 3. Fix the test or the code; red before, green after. 4. 500 runs each under load; macOS workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On 9f81f6018 (branch task-157), Docker arm64 (OrbStack, 15 CPUs) with stress -c 20 -i 4 -m 4 --vm-bytes 256M. The heaviest load runs 12 copies of the test binary at once. Both tests still existed and both still failed. Both causes are in the tests, not the code.
windows_share_bases_pages. CAUSE: it counted holders of the cached extent after the faults. A page-in's task (cache.rs read_part, self: Arc<CachedExtent>) still holds the extent after it notifies the fault it served, so the count came to 5, not 4. windows_share_extents_pages hit the same race and was fixed for it in 6e7b14359 (2026-10-18). RED: 42 of 300 under parallel load and stress, each one 'left: 5, right: 4' at extents.rs:3523. FIX (1e3a2f92c): count after the maps and before anything pages in, as the sibling test does. After the faults, check that each window maps the base's data and that the cache still holds the same extent. GREEN: nextest --stress-count 500 under stress, 500 of 500; 12 parallel x 42 under stress, 504 of 504.
guests_forks_failing_at_each_step_harm_nothing. CAUSE: each step's FailGuard ('1*return') is the process's, and the daemon's other stores upload on the default 1 s interval. An idle uploader's tick calls gather, which passes remote::gather. The other store's uploader took that one failure while the fork's store was still on its earlier steps, so the fork was made: 'left: (Ok(()), true)' at remote::gather. That uploader then stopped. Shown by logging the branches (temporary, not committed): all 31 failures were the other store's branch at remote::gather. RED: nextest --stress-count 20 under stress, 20 of 20 passed (too light to show it); 10 parallel x 4, 1 of 40 failed; 12 parallel x 6, 31 of 72 failed. FIX (8dc3b2e58): stores::uploads_when_asked(), test-only, makes the stores the daemon opens upload only when something waits for an upload: a fork, a sync, a commit stalled at the lag's bound, or a close. That is how remote::fork_faults' and restore_faults' stores upload (idle()). The test also waits (let_go, through open_paths) for each step's stores to close before arming the next step, because a store that closes uploads its last commit and checkpoints, passing writer::upload through writer::checkpoint. GREEN: 12 parallel x 42 under stress, 404 of 404 finished runs passed; the 12 runs in flight were cut off at the harness's 1 h limit (median run 71 s, longest 388 s, none hung). Then 12 x 9 more, 108 of 108: 512 of 512 in all.
BLIND SPOT: tests on the stores alone run their uploaders idle, so a one-shot failpoint there is only ever passed by the work under test. The daemon's tests open stores with the default interval, which none of them saw. LOOKED AROUND: the other one-shot failpoints in extentd/tests.rs (forks_whose_answers_are_lost_stand, control_forks_failing_fork_again, restores_over_what_a_dead_host_left_fail_until_it_is_deleted) arm daemon or fork-path steps (extentd::*, remote::forked, fork::committed, delete::*), which no idle uploader passes. fork_faults and restore_faults use idle options. No other test counts strong_count after a page-in (dax.rs:1497 and server.rs acks_tests have no page-ins).
STRUCTURE: the class is process-global failpoints shared by every store's uploader. Making it impossible would mean scoping a failpoint to one store, but the steps sit in three crates (extent_store, extent_remote, fxdax), many with no store in hand: too large for this, not done. Tests keeping the other uploaders quiet is the existing convention, which this follows. Making read_part drop its Arc before it notifies would not close the windows race: a second fault can find the page present before the task lets go, so counting before page-ins stays the right way.
OTHER RUNS: macOS clippy + workspace (FXDAX_TEST_PARTS='clippy workspace' scripts/test.sh), 2053 of 2053 passed. Linux -p fxdax in Docker, 448 of 455: 7 environmental failures, none touching this change (no sudo: grants x3; no eu-stack: stacks x3; no git for the build stamp: builds_are_stamped_with_their_commit). scripts/upstream-changes.py --check passes. Full Linux workspace and GCE not run. Mid-run the Mac's disk filled (ENOSPC) and OrbStack died; the runs it cut short were rerun, and the counts above are all from after.

GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes. Under load after landing with TASK-160.8 (let_go ported to the store table): 49/49 and 50/50 before the 1 h limit.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Both causes in the tests: windows_share_bases_pages counted holders while a page-in task still held the extent (counts before page-in now); guests_forks_failing_at_each_step_harm_nothing's one-shot failpoint was taken by another store's interval uploader (stores::uploads_when_asked, and each step waits for the last's stores to close). Under load: 42/300 failing -> 1004/1004 passing; 31/72 -> 512/512. GCE r584 on main c919610e7: Linux workspace 2249/2249 and every other part pass; macOS 2068/2068 + Shuttle on the landed stack; CI 41994971122; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
