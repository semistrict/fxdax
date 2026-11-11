---
id: TASK-365
title: >-
  extent_store::crash recovers_from_its_bucket_with_and_without_its_disk's
  coverage check fails under load: 20 commits spanned segments in 100 rounds,
  not more than 20
status: To Do
assignee: []
created_date: '2026-11-11 20:49'
labels:
  - extent_store
  - testing
  - flaky
dependencies: []
priority: medium
type: bug
ordinal: 749000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-11, main 04280b595, run 5 of a 30-run loop of cargo nextest run -p extent_store --test crash (under fxjob, the host loaded: an A/B's 16 VMs and other agents' jobs): crates/extent_store/tests/crash/main.rs:69, assert!(spanning > KEPT_ROUNDS / 5) with spanning = 20, exactly the bound. Every other check of the test passed (no round failed). How many commits span segments depends on how many writes land between the rounds' closings, which timing decides (a5dd824c5, TASK-92.41, added the check); a slow host makes fewer. Not a correctness failure, but it fails runs. Options: rounds that make a spanning commit by construction (write past a segment between commits) and count those, or a bound with margin taken from many runs' counts.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The check counts spanning commits the rounds make by construction, or its bound has a measured margin; it passes on a loaded host
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
