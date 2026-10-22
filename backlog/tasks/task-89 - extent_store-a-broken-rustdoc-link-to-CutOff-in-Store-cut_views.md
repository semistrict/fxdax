---
id: TASK-89
title: 'extent_store: a broken rustdoc link to CutOff in Store::cut_views'
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
priority: low
type: chore
ordinal: 263000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
cargo doc --document-private-items warns: Store::cut_views links CutOff, which isn't in scope there. Seen on main and on integrate-2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 cargo doc -p extent_store --document-private-items gives no warning
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
