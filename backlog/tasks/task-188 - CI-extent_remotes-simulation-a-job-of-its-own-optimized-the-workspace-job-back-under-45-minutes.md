---
id: TASK-188
title: >-
  CI: extent_remote's simulation a job of its own, optimized; the workspace job
  back under 45 minutes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 20:44'
updated_date: '2026-11-05 20:55'
labels: []
dependencies: []
references:
  - .github/workflows/ci.yml
  - scripts/test.sh
  - crates/extent_remote/src/sim/mod.rs
priority: high
type: task
ordinal: 641000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI's `test (workspace)` job took 42-51 minutes on main and was cancelled at its 90-minute limit on two branches on 2026-11-05. Most of what still ran were extent_remote's simulation tests (`sim::*`), built unoptimized inside the workspace part: the rule tests' seed searches (`sim::*_need_*`, each up to 20,000 seeds until the rule-off run finds a violation) took 1,174-1,619 s each (recoveries_need_newest_checkpoints, deletions_need_fences, takeovers_need_fences, collection_needs_records_listed_after_states), the profiles' 200-300-seed soaks 30-140 s each; 7,434 of the part's 10,500 test-seconds (GitHub run of landh, /tmp/landh-ws.log). Optimized, as agents run them, they take seconds to minutes. Separately, both runs spent 40 and 76 minutes in apt-get, the Azure mirror serving at 19-57 kB/s; most of what it fetched (ocaml, 130 MB of the 139) was coccinelle's, which only the scripts part uses.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/test.sh has a part `sim` running extent_remote's `sim::` tests optimized, with debug assertions and overflow checks kept; the workspace part no longer runs them
- [ ] #2 .github/workflows/ci.yml runs part `sim` as a matrix entry of its own; every test still runs in some job
- [ ] #3 Each rule test's seed search is bounded and fails naming its range when no seed finds a violation
- [ ] #4 The decision is under # Decisions in OPEN_QUESTIONS.md
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Cargo profile `sim` (release with debug-assertions and overflow-checks). 2. scripts/test.sh: the filterset `package(extent_remote) & test(/^sim::/)` once; workspace part excludes it, part sim runs it with --cargo-profile sim; sim in the default parts. 3. ci.yml on TASK-172 (9ff0b49bc, per-part packages): matrix entry sim with the common tools alone; scripts/tests/test_ci.py expects it. 4. Other SLOW tests of the logs (handover, extentd, fxfs-posix model/corrupt, extent_store::crash) checked: fixed-count workloads, not seed searches, 1-7 min in debug; stay. 5. Verify: parts sim, workspace, "clippy scripts" on macOS with timings. 6. OPEN_QUESTIONS.md decision.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
abd90f507 (on TASK-172's 9ff0b49bc, its per-part packages: part sim installs the common build tools alone; test_ci.py expects the part). macOS, M-series: FXDAX_TEST_PARTS=sim scripts/test.sh 74.8 s wall (build 33.6 s, tests 39.0 s, 47 passed, 4 marked leaky by nextest, passing; none leaky in CI's logs); slowest deletions_need_fences 37.6 s, recoveries_need_newest_checkpoints 37.3 s, collection_needs_records_listed_after_states 29.1 s, takeovers_need_fences 26.9 s (CI debug: 1,590, 1,619, 1,175, 1,217 s). FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast 326 s wall, 2,102 passed, no sim:: test run (65 skipped: the 47, and the ignored and Linux-only). FXDAX_TEST_PARTS="clippy scripts" 37 s, clippy clean, 163 scripts tests OK, mutants 17/4/38 apply. CI not yet run: the workspace part estimated from land-h at ~3,100 test-seconds without the simulation, about a quarter of an hour on 4 cores. Seed searches: already bounded at 0..20,000, panicking "no seed of 0..20000 needed the rule"; unchanged. Other SLOW tests of the logs (fxdax::handover, extentd's followers tests, fxfs-posix model/corrupt, extent_store::crash) are fixed-count workloads, not searches; they stay in the workspace part.
<!-- SECTION:NOTES:END -->
