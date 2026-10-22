---
id: TASK-92.37
title: >-
  Every removal during import scans every directory, saved time and path made so
  far
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
ordinal: 312000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
remove scans dirs, times and made in full, with a prefix test per key (import.rs:404-411), for every replacement, whiteout and opaque child (201-204, 185, 420-424), and made holds every path the layer has created (198, 205). A layer that replaces 30-50k files makes about 10^9 comparisons, several seconds, quadratic in the layer's size.

Where: crates/fxdax/src/import.rs:185, crates/fxdax/src/import.rs:198, crates/fxdax/src/import.rs:201-205, crates/fxdax/src/import.rs:404-411, crates/fxdax/src/import.rs:420-424.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. Importer::remove runs retain over all of dirs, times and made with a prefix test per key (crates/fxdax/src/import.rs:333-347). Callers: whiteouts (191), replacements of existing non-directories (204-205) and each opaque child (351-360). made holds every path the layer created (200, 210, 299; cleared per layer at 162). Not tracked. Related: TASK-39.5, TASK-79.56.
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
