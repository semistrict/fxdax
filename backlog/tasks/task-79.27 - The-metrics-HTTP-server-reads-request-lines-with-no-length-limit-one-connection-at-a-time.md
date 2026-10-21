---
id: TASK-79.27
title: >-
  The metrics HTTP server reads request lines with no length limit, one
  connection at a time
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 215000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
read_line has no length limit and the timeout applies to each read(), so any client that can reach the port grows a String until the daemon is OOM-killed and every VM stops. A slow client blocks all scrapes forever.

Where: crates/fxdax/src/extentd/metrics.rs:245, crates/fxdax/src/extentd/metrics.rs:234, crates/fxdax/src/bin/fxdax-extentd.rs:103.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-47. Not tracked. TASK-47 (Done) wrote the minimal HTTP/1.1 metrics responder that has the unbounded read_line and serves one connection at a time.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
