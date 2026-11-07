---
id: TASK-228
title: >-
  extent_store's durable_commits_keep_the_open_segments_pages_cached_on_the_host
  finds the open segment's raw file with a page not cached, once in about 2,000
  runs
status: To Do
assignee: []
created_date: '2026-11-07 09:36'
updated_date: '2026-11-07 09:44'
labels:
  - bug
  - test
dependencies: []
priority: medium
ordinal: 664000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen as land-k3's CI failure (run 42356848907, test (workspace): left 1, right 0 at crates/extent_store/tests/store.rs:1418). It flakes on main too: c9207e5d4 in Linux Docker failed once in 6 runs (--stress-count). Since TASK-165.9 a commit waits for no framing, so the test looked at the framed file while its framing still wrote it; waiting for Store::framing() to be empty first (land-k3) leaves it failing once in about 230 runs (300-run stress, Docker): a page of the framed file stays cached after its framing's sync, which ends with posix_fadvise(POSIX_FADV_DONTNEED) on the file after sync_data (crates/extent_store/src/dir.rs:450-459). Why that page stays (a page under writeback, a racing read, the file system) is not known. Production cost: page-cache memory alone.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test passes 2,000 runs in a row in Linux (stress), or what keeps the page is known and the test or the code says so
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: the framed-file half is fixed by waiting for the segment to settle (settled: closed and flushed), batch-x1's 9787e1af2, now on land-k3 too; waiting for Store::framing() to empty (my 01e4c786a) wasn't enough, failing once in about 230. Measured in Linux Docker, the test as on main: c9207e5d4 failed 6, 14 and 8 of 200 (28 of 600), land-k3 before (388df4a6d) 21, 5 and 11 of 200 (37 of 600), alternated: the same flake, not TASK-216's change (the test's store has no uploader). With settled: 400 of 400, then 1,999 of 2,000, the one failure the assertion before, store.rs:1415, the open segment's raw file with 7 of its 8 pages cached: a page evicted, or not yet written into the page cache, under load. That is what is left of this task.
<!-- SECTION:NOTES:END -->
