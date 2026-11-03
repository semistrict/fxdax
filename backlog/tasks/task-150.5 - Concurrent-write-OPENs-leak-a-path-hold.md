---
id: TASK-150.5
title: Concurrent write OPENs leak a path hold
status: Done
assignee: []
created_date: '2026-11-01 16:50'
updated_date: '2026-11-03 02:21'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 520000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. crates/fxdax/src/extentd/server.rs:1208-1231, with open_stores.rs:124-163: the store.get() check and the later set are not atomic, and failed sets are ignored. Two concurrent OPENs on one connection leave a path held, EBUSY until the daemon restarts, and one store without its flush-behind task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on Linux in cargo nextest run --workspace
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Confirmed: connections_open_one_store_at_a_time, deterministic. Fixed: an Opening guard is the only thing giving a connection its store, so the bad state can't be built, for open and follow. Blind spot as TASK-150.4. Class test: the fuzz reopens every path a case named, red with the bug back in about 70 s; with shrinking on a failing case ran 38 minutes unfinished. Structure: small, fixed. Branch fix/host-audit, 3c90f8f7b, merged locally; workspace on Docker arm64 only, 2070 of 2071 (the Docker-only failure), Shuttle 60 of 60, not yet on GCE.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Confirmed by a deterministic test and fixed (904dce663): only an Opening guard gives a connection its store, once, for open and follow alike, so concurrent OPENs can't leak a path hold (connections_open_one_store_at_a_time). The fuzz reopens every path a case named and went red with the bug back.
<!-- SECTION:FINAL_SUMMARY:END -->
