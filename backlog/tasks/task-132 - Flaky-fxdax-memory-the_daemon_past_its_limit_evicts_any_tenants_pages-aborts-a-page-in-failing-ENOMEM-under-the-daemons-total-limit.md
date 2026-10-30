---
id: TASK-132
title: >-
  Flaky: fxdax::memory the_daemon_past_its_limit_evicts_any_tenants_pages
  aborts, a page-in failing ENOMEM under the daemon's total limit
status: To Do
assignee: []
created_date: '2026-10-30 00:53'
updated_date: '2026-10-30 00:53'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 480000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r263 (buddy branch ccbc757e1, Linux workspace run): SIGABRT, 'reading ExtentId {..., sequence: 1} at 0x0: Errno(ENOMEM)' then 'unresolved DAX window fault at offset 0x0 (Read): ENOMEM'. Pre-existing: in OrbStack Docker, main (the commit before 'backlog: memory test flake filed') fails 2 of 12 runs of it alone, the buddy branch 1 of 12. A read fault's page-in under the 4-chunk total limit finds no memory where relief, evicting the other tenant's pages, should make room: likely relief freeing pages without freeing a whole chunk the starved arena could take, or the fault not waiting for relief. Kin of TASK-106 (tenants_past_their_limits_evict_then_refuse).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause found, and the test passing 50 runs in a row on Linux
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
