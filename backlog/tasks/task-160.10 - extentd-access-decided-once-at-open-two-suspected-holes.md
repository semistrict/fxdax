---
id: TASK-160.10
title: 'extentd: access decided once, at open; two suspected holes'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 581000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The default tenant is None, special-cased in four places; whether a store is a point, public base or follower is worked out by each caller. Suspected, unconfirmed: a control ResizeStore can change a point's capacity locally (ExtentStore::writable checks only followers; is_point is true only once an uploader is attached; stores::resize opens without a profile); and public_branches treats a read_dir error as 'no public branches', failing open, the base opened private and writable (extents.rs). Explains TASK-79.17, 150.4, 43.7, 154. Proposal: a real default Tenant with a policy, an Access fixed at open and enforced in ExtentStore's mutating methods, and reading the public markers failing the open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Red tests: ResizeStore on a point gets EROFS; a failing read_dir fails the open
- [ ] #2 No tenant.is_some() left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
