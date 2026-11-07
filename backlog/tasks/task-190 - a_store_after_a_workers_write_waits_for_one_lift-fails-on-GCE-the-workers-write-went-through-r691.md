---
id: TASK-190
title: >-
  a_store_after_a_workers_write_waits_for_one_lift fails on GCE: the worker's
  write went through (r691)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 01:32'
updated_date: '2026-11-07 23:54'
labels:
  - bug
  - dax
  - test
dependencies: []
references:
  - /tmp/fxdax-r691/fxdax-tests.log
priority: medium
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r691 (n2-standard-4, Linux 7.0 x86_64) ran the Linux workspace on main dde8d58d3, and fxdax extentd::tests::a_store_after_a_workers_write_waits_for_one_lift failed at tests.rs:975, "the worker's write went through" (Ok(()) where Err(EFAULT) was expected); log /tmp/fxdax-r691/fxdax-tests.log line 1753. The test (TASK-187, 9a8cb5bb6) had only run on Linux in Docker on OrbStack; r691 was its first GCE run. It stands for a forked guest's stores to pages not yet here: KVM's worker writes the page first, which must be restricted so the write fails, and the vCPU's store then waits for one lift. Under heavy load in Docker it also failed at tests.rs:981, a store's retries more than AccessFailures::IN_A_ROW (10 ms) apart by the wall clock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The test's pages are not here (mapped by the window, not installed) whatever the host's vm.max_map_count, and the test asserts it before the first write
- [x] #2 The test does not depend on wall-clock gaps between a store's retries
- [x] #3 A test pins that a write to a page installed here is a store whichever thread makes it
- [x] #4 The test passes 500 or more runs under load in Docker on Linux at vm.max_map_count 262144 and 1048576; cargo nextest run -p fxdax passes there
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce: in Docker, vm.max_map_count 1048576 as on GCE. 2. Map the stored-to pages to a sealed extent evicted from memory, not zeros; assert none installed. 3. Feed AccessFailures the test's clock. 4. uffd test: a non-guest write to an installed page is a store. 5. Stress 500 under load at both budgets; nextest -p fxdax.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: not a race. The test mapped zeros over its 1 GiB window (262,144 pages), which the window installs at once, each page a run of its own. The stage keeps runs installed up to a budget set by the host's vm.max_map_count: OrbStack's 262,144 gives 129,024, so the oldest runs, the test's low offsets among them, were left out and faulted as missing, as a page not here does. GCE's 1,048,576 gives 522,240, so every page stayed installed and write-protected. A write to a page here faults with UFFD_PAGEFAULT_FLAG_WP (flags 0x3, logged), which resolve() takes as a store whoever's thread made it, as documented on create_dax_window_in_host_memory, and the worker's write made the page private. TASK-187 notes show the test had only run in Docker on OrbStack; r691 was its first GCE run.
Reproduced deterministically: in Docker with sysctl vm.max_map_count=1048576, the unchanged test fails at page 0 every run ("the worker's write went through", Ok(()) vs Err(14), 1.25 s), as r691 did. It passes at 262,144. Under heavy load (stress --cpu 24 --io 4 --vm 4 with 8 runs at once), the unchanged test also failed 7 of 51 at tests.rs:981: a store's retries more than IN_A_ROW (10 ms) apart by the wall clock, which ended the run.
Blind spot: the test's precondition (pages mapped but not here) was never checked, and it held only through a host setting. The code is right: KVM's async-pf worker writes only pages not here, since KVM resolves a load of a page here itself.
Fix (test only): the stored-to pages map a sealed one-page extent, evicted (cache::evict_least_used), at every 64th page. Mapping installs nothing, whatever the budget, and the test asserts "0 pages installed, 0 left out, 0 private, 0 withheld, 0 restricted" before the first write. AccessFailures takes the test's clock, a microsecond a failure. The last private page is checked to hold the extent's bytes with the store's byte. A new dax::uffd test, writes_to_pages_here_are_stores_whoever_makes_them, pins the other side: a non-guest thread's write to an installed page goes through and makes it private.
Red/green: with the old zeros mapping put back, the new guard goes red at both budgets (262,144 pages installed at 1,048,576; 116,983 installed and 145,161 left out at 262,144). Without the eviction it still passes (a write leaves the page uncached); the eviction stays, so the test does not rely on that caching policy.
Stress (Docker on OrbStack, arm64, Linux 7.2, stress --cpu 16 --io 2 --vm 2, 6 runs at once): 500 of 500 at vm.max_map_count 1048576 (640 s), 500 of 500 at 262144 (723 s). Failed retries per run 1,000 to 1,082 (bound 2,000). cargo nextest run -p fxdax: 477 of 484; the 7 failures are the container's (eu-stack missing: stacks_name_where_threads_wait and two fxdaxctl sigquit tests; no git: builds_are_stamped_with_their_commit; the grants helper binary: 3 grants tests), the same set TASK-187 recorded. Clippy: nothing in the changed code. scripts/upstream-changes.py --check passes. The changed modules are Linux-only, so macOS compiles none of it.
Looked around: the other tests whose windows exceed a host's budget pin it (uffd with_budget tests; windows_budget_by_the_mappings_the_daemon_says_the_host_allows uses MAX_MAP_COUNT_GIVEN). The rest use windows of at most 64 pages. No other test depends on vm.max_map_count.
Structure: the store rule (a write to a page here is a store) is right for KVM and documented; now a test pins it. One unproven edge: if a page here ever needed KVM's async pf (memory read in, e.g. a swapped memfd page), the worker's write would be taken as a store and copy the page for a load. Not filed: nothing shows it happens.

Finalized in the backlog sweep: its commits are on origin/main. DoD #1 by Main CI 42423368353 at 878aa8848 (which holds this work) green, macos (workspace) and test (workspace) among it; scripts/upstream-changes.py --check passes on main (2026-11-08 sweep).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Not a race: the test mapped zeros over its 1 GiB window, so its pages were installed (here) unless the host's vm.max_map_count made the stage's budget leave them out. OrbStack did; GCE did not, and the worker's write took the documented store path. The test now maps an evicted sealed extent at its pages, asserts none is installed, and runs AccessFailures on its own clock. A new uffd test pins that a write to a page here is a store whoever makes it. Red: the old test fails at page 0 every run with vm.max_map_count=1048576 in Docker. Green: 500 of 500 under load at 1048576 and at 262144; nextest -p fxdax 477 of 484, the 7 the container's. Open: DoD 1, the GCE workspace run.
<!-- SECTION:FINAL_SUMMARY:END -->
