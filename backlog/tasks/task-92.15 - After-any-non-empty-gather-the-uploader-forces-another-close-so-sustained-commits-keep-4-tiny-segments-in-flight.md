---
id: TASK-92.15
title: >-
  After any non-empty gather the uploader forces another close, so sustained
  commits keep 4 tiny segments in flight
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 290000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A non-empty gather sets wanted = true and continues (remote.rs:608-610). The next gather calls close_now unconditionally (808), which closes after any commit (store.rs:2236), and with 4 uploads in flight wanted is never cleared (598-599). Ordered commits with no durable commit (builds, installs) then make about 40-50 closes and PUTs per second per store instead of about 1. Each close holds the commit lock through framing and syncs, and each segment adds marker fsyncs and a lease GET.

Where: crates/fxdax/src/remote.rs:598-599, crates/fxdax/src/remote.rs:608-610, crates/fxdax/src/remote.rs:808, crates/extent_store/src/store.rs:2236.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. A non-empty gather sets wanted = true and continues (crates/fxdax/src/remote.rs:622-625). wanted is cleared only when in_flight.len() < IN_FLIGHT (618-619), so with 4 uploads in flight it stays set. gather calls close_now unconditionally (777), and close_now closes after any commit (crates/extent_store/src/store.rs:1208-1210, has_commits). Related: TASK-91.4, TASK-79.19, TASK-91.3, TASK-61, TASK-28.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
