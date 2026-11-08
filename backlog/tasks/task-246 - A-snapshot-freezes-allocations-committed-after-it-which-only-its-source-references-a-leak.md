---
id: TASK-246
title: >-
  A snapshot freezes allocations committed after it, which only its source
  references: a leak
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-08 06:26'
labels:
  - bug
dependencies: []
priority: low
ordinal: 680000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left by TASK-241's fix. A transaction of a volume built before `snapshot_volume` froze the volume's data owner, held back by the commit gate (`LockKey::Commits`) and committed after the freeze, records its allocations under the frozen owner (its `Allocate` names the owner current when it was built). The snapshot doesn't reference those extents; only the source does, and a frozen owner's extents are never freed one by one (`deallocate`, `is_held_beyond_the_store`), so they stay allocated until no store shares the owner: space leaked, nothing lost or corrupted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An allocation committed after a snapshot froze its owner is the source's current data owner's, freed as the source frees it
- [ ] #2 A test of crates/fxfs commits an allocation held at the gate across a snapshot, frees it in the source, and finds its room back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
