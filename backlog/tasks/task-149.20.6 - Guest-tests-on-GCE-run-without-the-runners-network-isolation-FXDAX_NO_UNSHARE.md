---
id: TASK-149.20.6
title: >-
  Guest tests on GCE run without the runner's network isolation
  (FXDAX_NO_UNSHARE)
status: To Do
assignee: []
created_date: '2026-11-07 13:37'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: low
type: task
ordinal: 671000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. scripts/lib/linux-host-setup.sh sets FXDAX_NO_UNSHARE=1: buildah's re-execution fails on the GCE hosts (memfd_create: invalid argument), so guest/tests/runner runs each case without 'buildah unshare' and its network namespace, the isolation the harness was written with. Cases on one host share its network.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause of buildah unshare's failure on the hosts is known, and the isolation runs there, or the user decides it isn't needed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
