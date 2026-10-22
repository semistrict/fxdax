---
id: TASK-92.16
title: >-
  Segment count grows without bound, and every commit and every store open scans
  all segments
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
ordinal: 291000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd never cleans (clean's only caller is the virtio-fs path, image.rs:178), and close_now has no minimum size (store.rs:2236), so small, partly live segments accumulate. Each commit filters all of them under both locks (1907-1919). Each open reads every segment's frame (812-856), then reads the frame of each live own segment again (1033-1042), adding seconds to boot at thousands of segments on PD.

Where: crates/extent_store/src/store.rs:812-856, crates/extent_store/src/store.rs:1033-1042, crates/extent_store/src/store.rs:1907-1919, crates/extent_store/src/store.rs:2236, crates/fxdax/src/storage/image.rs:178.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. extentd never cleans: clean_after_commit's doc says the daemon's stores don't yet, and its only caller is the image device (crates/fxdax/src/host_store.rs:116-130; storage/image.rs:149-155). close_now has no minimum size. Each commit's Segments::reap filters every tracked segment (segments.rs:393-417). At open, close_left_open reads every segment's frame (segments.rs:652-690), then Segments::open reads each live own segment's frame again (segments.rs:112-135). Related: TASK-26.10, TASK-85, TASK-79.33, TASK-91.4, TASK-33.
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
