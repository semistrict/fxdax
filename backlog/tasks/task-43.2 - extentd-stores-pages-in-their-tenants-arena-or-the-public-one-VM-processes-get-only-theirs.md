---
id: TASK-43.2
title: >-
  extentd: stores' pages in their tenant's arena or the public one; VM processes
  get only theirs
status: To Do
assignee: []
created_date: '2026-10-18 04:00'
labels: []
dependencies:
  - TASK-43.1
parent_task_id: TASK-43
priority: high
type: feature
ordinal: 131000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Stores belong to a tenant or are public bases. The extent cache keys pages by arena, so a public base's sealed extents are paged in once into the public arena and shared by every tenant's forks; tenants' own extents go to their arena. A VM connection gets its tenant's memfd read-write and the public memfd read-only (reopened O_RDONLY), and the window maps public pages read-only.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Two tenants in one daemon: neither's memfd holds the other's pages (test)
- [ ] #2 Forks of a public base in two tenants share its pages: one copy (test)
- [ ] #3 Public pages are mapped read-only into windows; a write to one faults (test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
