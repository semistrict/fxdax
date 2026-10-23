---
id: TASK-92.16
title: >-
  Segment count grows without bound, and every commit and every store open scans
  all segments
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 23:09'
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
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done.  Audit: AC2 is not proven. 5513c144 removes the commit-side cost: a commit takes 0.4-0.5 µs at every segment count, against 46.8 µs at 20,000 before. The open-side cost is reduced, not removed or bounded, and the same store-bench segments measurement shows it. Opening reads 12.9 MB in 4.5 ms with 2,000 segments and 90 MB in 35 ms with 20,000, still linear: one serial block read per live closed segment. The segment count itself is still unbounded, which is the task's title. close_now has no minimum size, and cleaning takes only segments under live_fraction (segments.rs:101 is_victim). So small fully-live segments like the benchmark's are never merged, as the implementer's own 'partly-fixed' report says. No other backlog task tracks a bound on them. What AC2 lacks: a bound on the segment count or on open's per-segment reads (for example a minimum size for close_now, or merging small fully-live segments), shown by store-bench segments. Alternatively, the user can accept the reduction and move the rest into a new task. AC1 and DoD1-3 hold: the before and after are in the commit message and the round-1 report. branches_open_reading_a_block_of_each_closed_segment and segments_closed_between_commits_keep_their_raw_files_until_a_commit_after_is_durable pass on Linux (213 and 291 of 2544) and macOS (206 and 283 of 2466), and the crash tests pass on both. No changed file cites upstream.

User decision (2026-10-23): accept. The commit-side cost is removed (0.4-0.5 us per commit at any segment count, against 46.8 us at 20,000); opening's per-segment read stays linear (35 ms at 20,000 segments), and bounding the segment count moves to a follow-up task.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Commits now look over the empty segments alone, and opening reads one block of each closed segment rather than its whole footer. store-bench segments: a commit takes 0.4-0.5 us at every segment count, against 46.8 us at 20,000 before; opening reads 12.9 MB in 4.5 ms at 2,000 segments and 90 MB in 35 ms at 20,000. The user accepted opening's linear cost; merging small fully-live segments is a follow-up task. Verified by the segment tests and crash tests on macOS and Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
