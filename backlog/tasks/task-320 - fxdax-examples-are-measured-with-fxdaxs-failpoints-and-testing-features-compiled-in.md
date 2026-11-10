---
id: TASK-320
title: >-
  fxdax examples are measured with fxdax's failpoints and testing features
  compiled in
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 09:49'
updated_date: '2026-11-10 13:28'
labels:
  - bug
  - perf
dependencies: []
priority: medium
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/fxdax/Cargo.toml has a dev-dependency on itself with features ["failpoints", "testing"], and on extent_store with "testing". Examples are built with their package's dev-dependencies, so every fxdax example (extentd-mapbench, import-bench, store-read, store-bench, filesystem-io-bench, fxfs-metabench, pagein-bench) runs with fail/failpoints on, so each fail_point! in the daemon, extent_store and extent_remote looks up the fail crate's global registry under its RwLock, and with extent_store's `testing` lock instrumentation. Same class of leak as TASK-303 (the audit), which a release build now refuses; this one is not guarded. Found while fixing TASK-303.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A release build of a measured example has neither fail/failpoints nor extent_store/testing, shown by a build check or a guard like TASK-303's
- [ ] #2 The cost of the failpoints in a measured example is measured once (e.g. extentd-mapbench with and without), and the result recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Guards, as TASK-303's: extent_remote (the lowest crate with failpoints, which fxdax and extent_store depend on) refuses fail's failpoints (fail::has_failpoints(), a const fn, whoever turned them on) and its own testing in a build without debug assertions; extent_store refuses its testing likewise.
2. fxdax's examples move to a package of their own, crates/fxdax-examples, whose dependencies carry no test feature (fxdax's testing alone: import::testing, the images import-bench imports, no cost elsewhere). A package's examples build with its dev-dependencies, and fxdax's dev-depend on itself with failpoints and testing, which tests need.
3. scripts/test-e2e.sh builds a failpoints daemon (FXDAX_FAILPOINTS, FXDAX_FAULT_SEED, FXDAX_EXTENTD_FAILPOINTS) in the sim profile (release with debug assertions), a test's build.
4. Record which docs/measurements runs had them (fxdax examples since bee7356a6, 2026-11-01; extent_store/testing since 3afa08d64, 2026-11-08); re-measure those feeding an open decision (import-bench, TASK-92.47).
5. Measure the failpoints' cost once: extentd-mapbench as main builds it against the new package, on ubu1 (fxjob --bench).
6. Verify: release checks on macOS (fxdax-examples' examples build; -p fxdax --examples and a failpoints daemon in release refuse, with the message; sim builds); nextest workspace on macOS and ubu1; an e2e case with FXDAX_EXTENTD_FAILPOINTS=1 on ubu1.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10, what builds a measured binary with them, and since when:
- fxdax's examples (all nine): fxdax dev-depends on itself with failpoints and testing (failpoints since bee7356a6, 2026-11-01; testing alone before, which is only import::testing, the images import-bench imports) and on extent_store with testing (3afa08d64, 2026-11-08). A package's examples build with its dev-dependencies, so `-p fxdax --example` had both; main's extentd-mapbench has 35 fail::eval/panic_if_asked symbols, fxdax-examples' none.
- Workspace-wide builds (example-gce.sh until 70a12ea15): the same, unified across members; extent_store's dev-dependency on fail with failpoints too (2fdbfe03b, 2026-11-05).
- Not: test-e2e.sh's daemons (-p fxdax --bin, no dev-dependencies) but with FXDAX_FAILPOINTS/FXDAX_FAULT_SEED/FXDAX_EXTENTD_FAILPOINTS, which want them; fork-time (-p fxdax --bin); sqlite_extents' examples with -p (its dev-dependency on fxdax has no features); gcs-round-trips-2026-11-08 is a test (dev profile), by design.
Changes:
- Guards: extent_remote, the lowest crate with failpoints, refuses fail's failpoints in a build without debug assertions (const assert on fail::has_failpoints(), so whoever turned them on), and its own testing; extent_store refuses its testing (beside TASK-303's audit). Checked on macOS: release -p fxdax --features failpoints, -p fxdax --tests, -p extent_store/extent_remote --features testing, --workspace --examples all fail naming TASK-320; --profile sim failpoints daemon, -p fxdax-examples --examples, -p sqlite_extents --examples --bins, --workspace, -p fxdax --bins pass.
- fxdax's examples moved to crates/fxdax-examples, whose dev-dependencies carry fxdax's testing alone. fidl-fuchsia-io left fxdax's dev-dependencies (only fxfs-metabench used it).
- test-e2e.sh builds a failpoints daemon (and its fxdaxctl) in the sim profile. On ubu1: FXDAX_EXTENTD_FAILPOINTS=1 fxfs-guest-checks-dax built target/sim/fxdax-extentd and passed 1/1.
Measured runs that had them (note.md of each now says so): gce-import-bench-threads-{1,2}-2026-11-08 (failpoints), gce-sqlite-bench-2026-11-09T0955Z, T1038Z and gce-sqlite-commit-profile-2026-11-09T1111Z..T1223Z (failpoints and extent_store testing), ubu1-import-bench-threads-2026-11-10 (both). Of those only import-bench feeds an open decision (TASK-92.47, Options::executor_threads' default): re-measured as ubu1-import-bench-threads-2026-11-10b: two threads still about as fast (small files +3%, large file -10%); filesystem.rs' doc cites it.
Cost (AC #2): ubu1-mapbench-failpoints-2026-11-10, extentd-mapbench 10 runs each, alternating, fxjob --bench: main 1-3% slower on 12 of 15 request types (map 21.1 against 20.6 us, commit 733.5 against 718.8 us), within run-to-run spread; nothing on copy- or disk-bound ones.
Tests: macOS scripts/test.sh clippy, workspace (2210/2210), scripts pass. ubu1 (target on ext4): workspace 2461/2463, the two failures the host's: imports_read_what_they_made_from_the_hosts_cache (TASK-316, /tmp tmpfs under the cache floor) and images_dropped_as_their_pass_runs_leave_their_bucket_whole (80 s under load, passes alone in 13 s: filed TASK-328); with the target on ZFS, durable_commits_keep_the_open_segments_pages_cached_on_the_host fails as TASK-315 says. guest part 117+97+8 pass.
<!-- SECTION:NOTES:END -->
