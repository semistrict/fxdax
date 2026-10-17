---
id: TASK-35
title: 'Bench results count object-store requests and bytes, with their cost'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 15:44'
updated_date: '2026-10-17 16:03'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-pgbench-gcs
priority: medium
type: feature
ordinal: 104000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fs bench reports nothing about object storage when the extent daemon keeps stores in a bucket (FXDAX_EXTENTD_REMOTE): how many requests of each kind the uploader and fetcher made, how many bytes went up and down, and what that costs. After the pgbench run on GCS (docs/measurements/gce-fs-bench-pgbench-gcs) this was computed by hand from Cloud Monitoring and a download of the bucket. The bucket client should count its requests, extentd should log the counts, and the bench report should render them with an estimated cost.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extent_remote::store::Store counts its requests by kind (get, ranged get, head, list, create, update, overwrite, delete), failed ones included, and the bytes uploaded by kind of object (segments, extents of their own, checkpoints, other) and downloaded, shown by a test
- [x] #2 scripts/bench-report.py adds an object-storage section to comparison.md from extentd's log: requests per kind with their GCS class and estimated cost, and bytes up and down, shown by a test that scripts/test.sh runs
- [x] #3 fxdax-extentd keeps the counts in the file FXDAX_EXTENTD_STATS names (scripts/test-e2e.sh sets it), rewritten within 200 ms of each change, and logs them every 60 s while they change, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Stats in extent_remote::store (atomic counters, a snapshot with a one-line rendering), counting in each Bucket method.
2. extentd: a thread logging the line every 10 s when it changed, started with the bucket.
3. bench-report.py: parse the last line in logs/extentd.log; render the table and bytes; prices in one place with the pricing page and date.
4. Tests: Rust test of the counters on object_store's InMemory; Python unittest of the report, run by scripts/test.sh.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC2's 'from extentd's log' became its stats file (logs/extentd-stats.txt): the bench kills extentd right after a short case, before any periodic log line, so a file rewritten on change is what reliably holds the totals. Verified: extent_remote store::tests::counts_requests_and_bytes, fxdax extentd::tests::keeps_object_storage_stats (Lima), scripts/tests/test_bench_report.py (run by scripts/test.sh), and a bench in Lima (bench-fs-fxfs-guest-dax, fsync, FXDAX_EXTENTD_REMOTE=memory:///bench) whose comparison.md had the section. macOS 1966 + Linux 1977 tests pass. Found meanwhile, not fixed: the fxfs-guest-dax guest test times out with FXDAX_EXTENTD_REMOTE set (store attached, a few uploads, then the guest prints nothing).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The bucket client counts requests by kind (failed ones too) and bytes up by object kind and down; extentd keeps them in FXDAX_EXTENTD_STATS (set by test-e2e.sh) and logs them every 60 s; bench-report.py adds a section with requests, GCS class and estimated cost, and bytes. Verified by Rust and Python tests and an end-to-end bench in Lima.
<!-- SECTION:FINAL_SUMMARY:END -->
