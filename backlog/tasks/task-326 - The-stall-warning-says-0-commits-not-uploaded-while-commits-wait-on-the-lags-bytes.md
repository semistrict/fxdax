---
id: TASK-326
title: >-
  The stall warning says '0 commits not uploaded' while commits wait on the
  lag's bytes
status: To Do
assignee: []
created_date: '2026-11-10 11:54'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 727000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in TASK-317's backlog runs on ubu1: extentd logged 'commits wait for object storage: 1184763904 bytes not uploaded, 111022080 past the lag bound of 1073741824; 0 commits not uploaded, the oldest seen 5326 ms ago'. extent_store::upload::Shared::ask computes the commits from the epoch Remote::wait_lag passes, already less Options::max_lag, so a stall on bytes always says 0 commits; an operator reads that as nothing waiting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A stall on the lag's bytes says how many commits are not uploaded, counted from the store's latest commit
- [ ] #2 a_stall_says_what_it_waits_for covers a stall on bytes with commits behind
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
