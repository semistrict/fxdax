---
id: TASK-165.4
title: The part cache survives restarts and adopts whole local objects
status: To Do
assignee: []
created_date: '2026-11-05 03:16'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: medium
ordinal: 626000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 4. Once local disk holds no full copy, a host's bases live in the part cache; today it is wiped at start and its index is in memory, so an extentd restart would refetch every base. Acknowledged WAL segments must join it whole, without a copy.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Parts kept before a restart are read after it without a GET; the index is rebuilt from the files (their tails), each part checked against its CRC as it is read, a bad one dropped and refetched
- [ ] #2 Cache::adopt takes a whole framed object from a local file on the same filesystem (a rename), evictable from then on like any fetched part
- [ ] #3 Capacity and the free-space floor hold across restarts and adoptions (tests)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
