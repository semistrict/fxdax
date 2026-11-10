---
id: TASK-303
title: >-
  example-gce.sh measured every example with the extent store's audit compiled
  in
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 12:36'
updated_date: '2026-11-10 11:25'
labels:
  - bug
dependencies: []
priority: high
ordinal: 702000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/lib/example-linux-host.sh built examples with `cargo build --release --example NAME` at the workspace root, selecting every member, so that cargo unified features with extent_store's own dev-dependency on itself with `audit`: the store recounted its state after each critical section in every example run on GCE (sqlite-bench, sqlite-commit-profile, extentd-mapbench, store-read and others). Found by perf on 2026-11-09 (TASK-301): State::audit, KeptExtents::audit and Recount took ~8% of a one-row commit's CPU. Fixed in the same change (-p the example's package). Earlier example measurements in docs/measurements include that cost.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 example-linux-host.sh builds the example's package alone, so that no audit is compiled into a measured example
- [x] #2 docs/measurements notes of example runs before the fix say they ran with the audit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find what still builds measurement binaries: example-gce.sh (GCE retired), bench.sh/bench-ab/perf gate (test-e2e.sh builds -p fxdax --bin, no dev-deps), fork-time (-p fxdax --bin).
2. Guard: extent_store refuses to compile with audit and without debug assertions (release = measurement profile; dev and sim test profiles keep it).
3. extent_store's own example store-bench always had the audit (self dev-dependency): move it to fxdax's examples.
4. Retire example-gce.sh and lib/example-linux-host.sh (GCE gone; the host half provisions a disposable root host, unfit for shared ubu1; with the guard a plain cargo run --release -p PKG --example is safe).
5. Mark the example runs measured with the audit (note.md of each, 2026-11-08..09); re-measure import-bench 1 vs 2 threads on ubu1, which TASK-92.47's open decision and Filesystem::executor_threads' default rest on.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10: What still builds measured binaries, and whether the audit reached them:
- example-gce.sh (scripts/lib/example-linux-host.sh): every example run on GCE; GCE is retired. Fixed by 70a12ea15 (-p), but extent_store's own example, store-bench, always had the audit: a package's examples build with its dev-dependencies, and extent_store dev-depends on itself with audit.
- bench.sh, bench-ab-*.sh, perf-gate-gce.sh, pgbench and fs-bench cases: through scripts/test-e2e.sh, which builds -p fxdax --bin fxdax-extentd --bin fxdaxctl and -p extent_remote --bin fxdax-gc: no dev-dependencies, no audit. fork-time-linux-host.sh: -p fxdax --bin, none. Measured daemons never had it.
- Guard: extent_store refuses to compile with audit and without debug assertions (compile_error!): release is the profile measurements use; tests run in dev or sim (release + debug assertions). Checked on macOS: cargo check --release --workspace --examples fails with the message (the old bug); --release -p fxdax --examples --bins, -p sqlite_extents --examples --bins, --release --workspace (libs, bins), dev and sim extent_store tests all pass.
- store-bench moved to crates/fxdax/examples (it uses only extent_store, storage_device, tempfile).
- example-gce.sh and its host half retired rather than ported: GCE is gone; the host half provisions a disposable root host (apt, /var/lib/fxdax-test), unfit for ubu1, which agents share; and with the guard a plain cargo run --release -p PACKAGE --example NAME is safe on ubu1, as ubu1-sqlite-2026-11-09 was run.
- Runs measured with the audit (2026-11-03 2024404bd, until 70a12ea15), their note.md now say so: gce-import-bench-threads-{1,2}-2026-11-08 (TASK-92.47), gce-sqlite-bench-2026-11-09T0955Z and T1038Z (TASK-298, TASK-300), gce-sqlite-commit-profile-2026-11-09T1111Z, T1114Z, T1156Z, T1208Z, T1212Z, T1217Z, T1221Z, T1223Z (TASK-301). Not: T1236Z and T1241Z (their source had the fix), ubu1-sqlite-2026-11-09 (-p sqlite_extents). Examples before 2026-11-03 predate the audit.
- Of those, only import-bench feeds an open decision (TASK-92.47, and Filesystem::executor_threads' default of 1, whose doc cites it): re-measured on ubu1 below. TASK-298/300/301 are done; TASK-301's later numbers (T1236Z, T1241Z, ubu1) were without it.
- Found: fxdax's examples are built with fxdax's own failpoints and testing features (its dev-dependency on itself): TASK-320.

Main CI 42804968972 green at f09dbd657.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A release build with extent_store's audit now fails to compile (release is what measurements use; tests run in dev and sim). Measured daemons never had it (test-e2e.sh and fork-time build -p fxdax --bin). store-bench, which always had it as extent_store's own example, moved to fxdax. example-gce.sh and lib/example-linux-host.sh are retired (GCE gone; the host half provisioned a disposable root host). The 12 example runs measured with the audit say so in their note.md. import-bench, behind TASK-92.47's open default, was re-measured on ubu1 without the audit: two Fxfs threads still no faster. Verified with release and dev/sim cargo checks on macOS, a release example build on ubu1, and main CI 42804968972. Found TASK-320 (failpoints and testing in fxdax's examples).
<!-- SECTION:FINAL_SUMMARY:END -->
