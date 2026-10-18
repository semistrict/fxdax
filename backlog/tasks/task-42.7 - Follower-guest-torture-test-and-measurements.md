---
id: TASK-42.7
title: Follower guest torture test and measurements
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 04:10'
labels: []
dependencies:
  - TASK-42.6
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 127000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The only test of the kernel side under real concurrency (doc-8 testing 6 and 7), as Linux's rcutorture: a writer VM and followers on a debug kernel, with self-checking workloads and delay injection, for hours.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-guest-follow: a writer and two or more followers, DAX on and off, with self-checking workloads (a token renamed between directories, per-block generations and checksums, mmapped readers under truncate/extend/rewrite, files deleted while open), histories checked by TASK-42.1's checker
- [ ] #2 A debug guest kernel (KCSAN, lockdep, DEBUG_ATOMIC_SLEEP) and delay injection points (between apply and invalidation; between iomap_begin and the PTE insert) run it clean
- [ ] #3 Mutation checks: each delay point with its invalidation skipped makes it fail
- [ ] #4 Hours on GCE pass; the writer's commit latency and pgbench tps unchanged with followers attached; followers' lag and pinned bytes recorded in docs/measurements
- [ ] #5 A writer mount with no followers runs pgbench, the fs bench's metadata and io groups, and commits as fast as on the commit before the follower code (within noise), the follower paths gated per mount at run time (a static key), not compiled out
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
