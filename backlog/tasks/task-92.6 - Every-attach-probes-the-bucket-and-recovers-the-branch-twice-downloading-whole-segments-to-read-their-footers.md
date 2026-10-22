---
id: TASK-92.6
title: >-
  Every attach probes the bucket and recovers the branch twice, downloading
  whole segments to read their footers
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
priority: high
type: enhancement
ordinal: 281000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Shared::start runs probe on every attach (remote.rs:707): 4 conditional PUTs and a DELETE in sequence (store.rs:371-394), though the bucket is a process singleton (stores.rs:613-632) and doc-7:239 says startup. start then recovers the branch (remote.rs:712), and Writer::open reads the record and recovers again (writer.rs:116-125). Each recovery GETs every post-checkpoint segment whole and serially, only to read its footer (reader.rs:168-170, 252-254, 195-196). Every VM boot, fork child, follower and restore point pays this (stores.rs:307): about 0.15-0.3 s of probe plus at least 5 round trips, out of a 1.16 s open. After a crash, up to 63 whole segments (up to 2 GiB) are downloaded twice.

Where: crates/fxdax/src/remote.rs:707-731, crates/extent_remote/src/store.rs:371-394, crates/extent_remote/src/writer.rs:116-125, crates/extent_remote/src/reader.rs:168-170, crates/extent_remote/src/reader.rs:195-196, crates/extent_remote/src/reader.rs:252-254, crates/fxdax/src/extentd/stores.rs:307, crates/fxdax/src/extentd/stores.rs:613-632.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. Shared::start probes the bucket on every attach (crates/fxdax/src/remote.rs:680; probe at extent_remote/src/store.rs:391-414: three conditional PUTs and a DELETE in sequence). It recovers the branch (remote.rs:685), then Writer::open reads the record and recovers again (writer.rs:81-91). Recovery GETs each post-checkpoint segment whole and serially, only to read its footer (recovery.rs:170-193, footer at 196-205). TASK-79.4 (attach diffs from a recovery made before the lease) touches the same start/Writer::open hand-off. Related: TASK-79.4, TASK-90, TASK-43.6.
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
