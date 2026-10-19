---
id: TASK-49
title: >-
  StatStore: a store's bytes split into its own and those it shares with its
  parents
status: To Do
assignee: []
created_date: '2026-10-19 11:13'
labels: []
dependencies: []
priority: high
ordinal: 148000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Per-tenant storage billing needs what each store costs alone. StatStoreResponse has capacity, used and available: used counts every extent the store holds, including those a fork inherited from its parents and still shares with them, so forks of one base are billed for the base's bytes again and again.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 StatStore reports, besides used, the bytes of extents only the store holds and the bytes of those it shares with the branches it was forked from
- [ ] #2 Both are exact for a base, a fork, and a fork of a fork, as extents are written, freed and inherited: a test checks each
- [ ] #3 The split costs StatStore no read of the store's extents' data, and stays correct after the daemon restarts
- [ ] #4 fxdaxctl stat prints both
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
