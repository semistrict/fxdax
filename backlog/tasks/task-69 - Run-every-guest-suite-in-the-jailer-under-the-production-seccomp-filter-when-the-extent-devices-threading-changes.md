---
id: TASK-69
title: >-
  Run every guest suite in the jailer under the production seccomp filter when
  the extent device's threading changes
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
priority: medium
ordinal: 178000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
0a6f1e33 moved request completion onto the reply thread; only fxfs-guest-dax ran under the production filter. Rerun all 26 suites in the jailer whenever the extent device's threading changes (and now).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: every suite of scripts/test-e2e.sh's default list, in the jailer under the production seccomp filter (real) with branches in real GCS: r353, all but four; r355, those four after TASK-142's fix and a remote run's 360 s for the checks: fxfs-guest-pgbench, checks-dax, -delay, -small. .github/workflows/nightly.yml runs this nightly, once its secrets are set.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every suite of scripts/test-e2e.sh's default list ran in the jailer under the production seccomp filter with branches in real GCS: r353, then r355 for the four left after TASK-142's fix. .github/workflows/nightly.yml runs this nightly; it never parsed until a1e079306, and needs the GCP credentials OPEN_QUESTIONS.md asks for.
<!-- SECTION:FINAL_SUMMARY:END -->
