---
id: TASK-261
title: >-
  Guest tests: the run-wide extentd's part cache is on tmpfs (/tmp), its stores
  on disk: every adoption into the cache fails EXDEV, ~14,000 warnings a run
status: To Do
assignee: []
created_date: '2026-11-08 14:09'
labels:
  - tests
  - harness
dependencies: []
priority: medium
ordinal: 691000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/test-e2e.sh puts the run-wide daemon's FXDAX_EXTENTD_CACHE in ${TMPDIR:-/tmp} ("on disk, as production's daemon does", TASK-164.12), but on the GCE hosts /tmp is a tmpfs, while the stores' directories are under the run's results on the root ext4 disk. Cache::adopt renames a leaving segment or extent file into the cache, so every adoption fails with EXDEV (os error 18), the file is removed instead, and extentd.log carries one WARN per file: val-A3 (run-20261108T120712Z-567f2024, 08d444ef7, fxdax-test-r956-k6) has 14,304 "extents/... into the cache ... Invalid cross-device link" and 7,173 "segments/..." lines, full-c (096fa2233) 9,086 and 4,680. So the guest suites never exercise adoption (TASK-165.4/165.8) as production does, and the warnings bury the log's real ones. Found investigating xattrs_bounded/append_writeback budget overruns.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The run-wide daemon's cache is on the same filesystem as its stores on the GCE hosts and in CI, or the harness says why not
- [ ] #2 A guest-suite run's extentd.log has no "into the cache ... Invalid cross-device link" warnings
- [ ] #3 Adoptions into the cache happen in a guest-suite run (extentd stats or a check shows them)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
