---
id: TASK-172
title: >-
  A jailed VMM sizes its DAX run budget from vm.max_map_count's default, not the
  host's: it can't read /proc
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:06'
updated_date: '2026-11-05 20:46'
labels:
  - dax
  - jail
  - perf
dependencies: []
priority: medium
ordinal: 635000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
uffd.rs's default_budget reads /proc/sys/vm/max_map_count, which the jailer's chroot lacks, and falls back to 65,530: a budget of 30,717 runs where the GCE hosts' 1,048,576 gives 522,240. The jailed VMM trims runs about seventeen times as often as an unjailed one, which made TASK-170's leak common (21,396 pages in one pgbench run) and costs faults. Second jail-only /proc dependency after TASK-167 (thread names): the VMM's view of the host shouldn't depend on /proc it may not have.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A jailed VMM's budget is the host's, as an unjailed one's is, or a configured one (e.g. given in the extent device's configuration or by the jailer's caller)
- [x] #2 A test shows the budget without /proc is what was given, not the default
- [x] #3 No other read of /proc in the VMM's DAX code silently falls back in the jail (audit uffd.rs and the extent device)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. The extent daemon, unjailed on the VM process's host, reads vm.max_map_count (physmem::max_map_count, an error without /proc rather than a default) and tells each VM process it in OPEN's reply (Opened.max_map_count, args[3]).
2. The client sizes its window's run budget from what the daemon said (UffdSecondStage::new takes max_map_count; uffd's default_budget, its /proc read, goes); the VMM logs the budget at info.
3. uffd's thread_name says 'unnamed: no /proc' rather than 'gone' without /proc (AC3 audit: the only other /proc read in the VMM's DAX code).
4. Test: extentd windows_budget_by_the_mappings_the_daemon_says_the_host_allows, the daemon saying 300,000; red with the client reading /proc.
5. Verify: macOS scripts/test.sh, Linux Docker, jailed GCE fxfs-guest-pgbench-dax-forked and fxfs-guest-checks-dax with FXDAX_FIRECRACKER_LOG_LEVEL=Info showing the budget.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Structure: the VM process's view of the host is the daemon's to give. The daemon runs unjailed on the same kernel (it shares its memfds with the VM process), vm.max_map_count is a global sysctl, and OPEN's reply already hands the VM process what it needs of the host (its physical memory): args[3] now carries the host's max_map_count (Opened.max_map_count). uffd.rs's default_budget, its /proc read and silent 65,530 fallback, is gone: UffdSecondStage::new takes max_map_count, and budget() is a pure function of it. physmem::max_map_count is the one reader of the sysctl, an io::Error without /proc rather than a default; physmem::max_size and the daemon use it (the daemon warns and tells Linux's default if even it can't read /proc). Jailer or Firecracker arguments were the alternative: VMM-specific, and a second path to keep in step; the daemon serves any VMM.

Blind spot: the jail was never a test environment for the VMM's host reads; the budget was nowhere observable but the failure dump (describe_windows). Test for the class: the budget now comes only from what the daemon says, so a test daemon saying 300,000 pins it (extentd::tests::windows_budget_by_the_mappings_the_daemon_says_the_host_allows, red with the client reading /proc itself: 129,024 from the container's 262,144, against 147,952); and the VMM logs its window's budget at info, which jailed GCE runs check.

AC3 audit: the VMM-side DAX code (uffd.rs, extentd/client.rs, Firecracker's extent device) reads /proc only in uffd.rs's thread_name, for refusal logs; TASK-167 already classifies by the VMM's own registry. Without /proc it said 'gone' for every thread; it now says 'unnamed: no /proc'. Firecracker's vmm crate reads no /proc (a comment in regs.rs). The daemon's /proc reads (server.rs's /proc/self/fd, extent_store's) run unjailed.

Verified at 3da9444bd's tree: Linux (Docker, OrbStack, privileged) the new test and every uffd test, 16/16, the new test red with the client reading /proc (129,024 against 147,952); macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast, workspace 2120/2120, Shuttle all pass; GCE r631 (us-central1-a) workspace 2302/2302 and every other part green; GCE r630 (us-east4-a) jailed (FXDAX_FIRECRACKER_JAIL=1, seccomp real, log level Info): fxfs-guest-pgbench-dax-forked and fxfs-guest-checks-dax pass, unexplained-refusals empty, each VMM logging 'a DAX window of N bytes keeps at most 522240 runs of pages installed, the host letting a process make 1048576 mappings'; scripts/upstream-changes.py --check passes.

2026-11-05, CI run 42089880804 (land-h on 7c175bc91): the workspace job was cancelled at 90 min a second after its suite passed, 2317/2317 (Summary 20:31:51.79, cancel 20:31:56). apt-get took 40 min of it at 57 kB/s from the Azure mirror, mostly Coccinelle and OCaml. Not a hang: handover's stores_open_whole... took 408 s there and 382 s in main's run 42084347494; takeovers_need_fences 1,217 s and 946 s. Alone in Docker (arm64), five runs each, handover 14.5-16.4 s on this branch, 11.5-20.7 s on origin/main ce9d5b2f2; takeovers_need_fences 161.4 s and 161.8 s. Fixed in CI: each part installs only its own tools (9ff0b49bc, scripts/tests/test_ci.py, red on the old workflow). Rebased onto ce9d5b2f2: fxdax and physmem in Docker 522/526, the 4 the container's (grants tests' spawned tool missing, ENOENT; the commit stamp 'unknown' without .git), as TASK-165.1 recorded.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The extent daemon, unjailed on the VM process's kernel, reads vm.max_map_count and tells each VM process it in OPEN's reply (args[3]); the VM process's DAX window budgets its runs from that, never from /proc, which its jail lacks. physmem::max_map_count is the sysctl's one reader and fails without /proc instead of defaulting; uffd's refusal logs say 'unnamed: no /proc' rather than 'gone' in a jail. Test windows_budget_by_the_mappings_the_daemon_says_the_host_allows (red before: the client's own /proc read). Verified: macOS workspace and Shuttle; Linux Docker; GCE r631 workspace 2302/2302; GCE r630 jailed pgbench-dax-forked and checks-dax pass with the VMM logging a budget of 522,240 runs of 1,048,576 mappings.
<!-- SECTION:FINAL_SUMMARY:END -->
