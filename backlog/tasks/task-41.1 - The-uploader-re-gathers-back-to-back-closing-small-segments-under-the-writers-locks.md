---
id: TASK-41.1
title: >-
  The uploader re-gathers back to back, closing small segments under the
  writer's locks
status: To Do
assignee: []
created_date: '2026-10-22 08:54'
labels:
  - perf
dependencies: []
parent_task_id: TASK-41
priority: medium
ordinal: 167000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-42.11's diagnosis (2026-10-21, GCE c3-standard-8, pgbench, main 0a3015b with counters in a scratch tree): crates/fxdax/src/remote.rs's uploader, after a gather that found segments, sets wanted and gathers again at once ('More may have been made durable meanwhile'), until IN_FLIGHT uploads are in flight, and again as each completes. Each gather flushes the store, then Store::close_now, which holds the store's committing lock and its state while it frames the segment inline (now = true) and fsyncs it and its directories, so the writer's writes and commits wait meanwhile. Writer alone: 200 gathers (91 empty, 107 of them re-gathers), 143 segments closed, 46 ms framing each. With two idle followers: 1,000 gathers (917 re-gathers, 72 empty, 770 waiting with every upload in flight), 1,010 segments of ~100 KiB, flush before close 7 ms each against 5 ms. Dropping the re-gather (waiters that ask meanwhile set wanted themselves) brought the idle followers' case back to 146 segments; remote's unit tests passed without it. Writer alone, close_now's inline framing under the locks is some 1.1-1.4 s of a pgbench run's commits held up.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A gather is made only when a commit waits for its upload, the interval is due, or a fork or stop asks; or the re-gather is shown to be needed, with why
- [ ] #2 close_now frames its segment and syncs off the writer's locks, as the commit path's closes do (Framing), or the cost of holding them is measured as negligible
- [ ] #3 pgbench on fxfs-guest-dax measured before and after on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
