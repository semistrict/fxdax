---
id: TASK-149.20
title: Audit what the tests exclude
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 16:54'
updated_date: '2026-11-07 13:55'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-15 - What-the-tests-exclude-and-why.md
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 537000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
test_short_copy already covered a data loss bug but was switched off on DAX. Nobody reviews what is excluded, and exclusions outlive their reasons.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A list of every if !dax, skipped check, ignored test and xfstests known failure, each with why
- [x] #2 What no longer has a reason runs again, its failures filed
- [x] #3 A new exclusion must say why, checked by scripts/check-guest.sh or the like
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Inventory every exclusion: #[ignore] and cfg_attr ignores, cfg/feature gates, nextest filters and test.sh/CI parts, known-failure lists (xfstests, pjdfstest, guest/sim, fxfs-posix), FXDAX_* switches, guest cases in no suite (Tag::Manual), checks run in some suites alone, ShouldRun::No, mutants with Killer: none, .cargo/mutants.toml.
2. For each: still justified (reason and task named), or stale (run it again, fix what fails or file it). Run what nobody runs that Docker can: the S3 and Azure tests against RustFS and Azurite in containers.
3. Fix what was found: fxdaxctl take's lease token; test-rustfs.sh and test-azurite.sh; the branch tests on a file bucket in every run; xfstests' list failing when a listed test passes; reasons in every fxdax #[ignore]; patch-0012's why.
4. AC #3: scripts/check-exclusions.py in the scripts part, with tests; CI job object-stores.
5. File gaps (TASK-149.20.1-6); the list as doc-15.
6. Docker and macOS runs; push land-h5 with CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: The list, each exclusion with its reason and verdict: doc-15. Kept, reasons named: benchmarks, child processes, upstream Fuchsia's ignores, the sim split, the firecracker filter, Linux-only gates, Manual guest cases, ShouldRun::No's, xfstests' known failures (TASK-143/144/145 open), five equivalent mutants.
Stale, run again: the S3 tests (RustFS) and the Azure one (Azurite), which nothing ran: scripts/test-rustfs.sh ran scripts/test.sh with --run-ignored only (every part) and needed a RustFS built from source; test-azurite.sh took a podman installed but not running, and its probe gave up on an empty reply. Both now run their server in a container (scripts/lib/containers.sh, RustFS 1.0.4, Azurite 3.37.0) and CI's new object-stores job runs them. Run against RustFS, rustfs_places_and_syncs_stores failed: a real bug, fxdaxctl take DIR CAP BRANCH TOKEN of a new copy took the branch's lease with the host's token (1) as it made the store, before its placement's (6), so a later placement couldn't take a branch over (ESTALE, the lease at 5). Fixed in ExtentStore::take_with; the test's expectation updated for TASK-165.9 (an earlier placement is refused at take, not at sync). The two branch tests now also run in every suite run on a file bucket (forks_and_takes_branches, places_and_syncs_stores).
Fixed: every fxdax #[ignore] says why; patch-0012-invalidate-unmaps-after's header says why it has no killer (it was in guest/sim/README.md alone); xfstests' list now fails the case when a listed test runs and passes (but 301/302, which pass or not), unit-tested on the host.
AC #3: scripts/check-exclusions.py (scripts part): a bare #[ignore] of fxdax's, a known-failure line naming no open task, an xfstests known failure under no comment, a Killer: none saying no why, each fails; 9 tests.
Filed: TASK-149.20.1 xfstests in no suite; .2 no macOS CI; .3 the page-cache checks not on DAX mounts, whose new files are in the page cache (stale premise, GCE to run); .4 no oracle for a region's put unmapping less than the host maps (task-44.19's mutant); .5 the GCS test runs nowhere; .6 FXDAX_NO_UNSHARE on GCE.
Runs: macOS workspace + shuttle all pass (2,105); Docker Linux workspace 2,326 of 2,330 (the container's 4: git, sudo), guest part 202 of 202, clippy; scripts 188 of 188 on macOS (in the container 8 fail for its lack of git, rsync and Coccinelle); RustFS and Azurite tests pass locally in containers; mutants --check all; upstream-changes.py --check passes. DoD #1 open: Lima not run (Docker instead).
<!-- SECTION:NOTES:END -->
