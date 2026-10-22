---
id: TASK-92.22
title: >-
  NVMe cache hits need the object's tail, held in a 4096-entry map that is
  cleared all at once
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
ordinal: 297000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
kept() must await the object's tail before it can use a cached part (fetch.rs:426). The tail map is capped at 4096 and cleared entirely when full (161, 454-456), and the cache index keeps no part descriptor (cache.rs:33), so a tail miss costs a suffix GET (472). An adopted or restored store whose reads cycle over more than 4096 objects pays 20-40 ms of GCS before nearly every first cache hit after a clear, 20-40x a ~1 ms hit.

Where: crates/extent_remote/src/fetch.rs:161, crates/extent_remote/src/fetch.rs:426, crates/extent_remote/src/fetch.rs:454-456, crates/extent_remote/src/fetch.rs:472, crates/extent_remote/src/cache.rs:33.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. kept() awaits self.tail(key) before it can use a cached part (crates/extent_remote/src/fetch.rs:445-447). TAILS=4096 (fetch.rs:181), and tail() clears the whole map when full (fetch.rs:474-476). The cache index stores only (offset, len, used) per part, with no stored descriptor (cache.rs:33). So a tail miss still costs a suffix GET (object::parts, fetch.rs:479). Not tracked. Related: TASK-28.9, TASK-29.3.
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
