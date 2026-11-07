---
id: TASK-228
title: >-
  extent_store's durable_commits_keep_the_open_segments_pages_cached_on_the_host
  finds a framed segment's page cached, rarely: fadvise after sync leaves one
status: To Do
assignee: []
created_date: '2026-11-07 09:36'
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
