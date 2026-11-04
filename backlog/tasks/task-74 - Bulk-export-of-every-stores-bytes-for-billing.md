---
id: TASK-74
title: 'Bulk export of every store''s bytes, for billing'
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-04 07:06'
labels: []
dependencies: []
priority: low
ordinal: 183000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
StatStore has own and inherited bytes, one call per store. Add a bulk export of every store's bytes.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
ListStores returns every store's StatStore, own and inherited bytes included, in one call (TASK-50.5).
<!-- SECTION:FINAL_SUMMARY:END -->
