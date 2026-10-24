---
id: TASK-79.99
title: >-
  extent_store: failed flush leaves later pending framings running after branch
  close
status: To Do
assignee: []
created_date: '2026-10-24 00:54'
labels:
  - review-2026-10-23
dependencies: []
references:
  - crates/extent_store/src/store.rs
  - crates/extent_store/src/segments.rs
  - crates/extent_store/tests/store.rs
  - crates/extent_store/tests/shuttle/main.rs
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 369000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-10-23 adversarial review at bce42647. Severity P2; high static confidence, not dynamically reproduced. Data loss is not established: demonstrated risk is recovery availability and branch ownership ordering. Store::make_durable waits for framings within one Pending, then framed? returns on failure without waiting later Pending commits in the flush batch (crates/extent_store/src/store.rs:1585-1604). Closing has no joining destructor (segments.rs:811-821). flush_locked drops the taken batch on failure (store.rs:1562-1566); close only waits state.framings and state.closings, excluding discarded pending framings (store.rs:625-638). Trigger: two ordered commits close segments; an earlier framing fails while a later one remains in flight. Branch close can release ownership while the old job still creates or writes its segment (segments.rs:880-896). Recovery removes and recreates that path (segments.rs:1023-1032), so a competing create can cause EEXIST or expose an incomplete frame. HostDir::create uses create_new (dir.rs:182-189); an overwrite/data-loss claim is unjustified. Tests cover individual framing failures and close_now/flush waits, not this multi-Pending error path. Related TASK-79.33, TASK-79.69, TASK-79.80 concern different framing failures. Estimated fix effort medium; fix risk low to medium.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 After a framing failure, every already-started framing in the whole taken flush batch finishes before branch ownership is released, while the original error remains reported.
- [ ] #2 A deterministic regression holds a later pending framing and fails an earlier one; close retains branch ownership until the held framing ends. This correct-behavior expectation fails before the fix.
- [ ] #3 After the failed store closes, reopening has no stale framing job racing recovery and recovers the durable commit prefix with surviving data intact.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 The fix batch passes cargo build or cargo check, cargo clippy, scripts/test.sh on macOS and scripts/test-gce.sh for the Linux workspace and applicable guest cases.
- [ ] #2 Changed upstream deviations are documented, scripts/upstream-changes.py --check passes, and generated files are updated through their generators.
<!-- DOD:END -->
