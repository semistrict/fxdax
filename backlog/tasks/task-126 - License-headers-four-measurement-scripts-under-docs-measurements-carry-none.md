---
id: TASK-126
title: 'License headers: four measurement scripts under docs/measurements carry none'
status: To Do
assignee: []
created_date: '2026-10-29 01:15'
labels: []
dependencies: []
ordinal: 459000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/license-headers.py fails on main: docs/measurements/gce-fs-bench-sqlite-gap-2026-10-25-final-control/frozen-host.sh, docs/measurements/gce-fs-bench-sqlite-gap-2026-10-25-host-profile2/profile-host.sh, docs/measurements/sqlite-dax-appends-2026-10-24-validation/ab-repeat.sh and ab.sh have no fxdax copyright or BSD notice. Found while checking TASK-96.1 branch; not caused by it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/license-headers.py passes on main
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
