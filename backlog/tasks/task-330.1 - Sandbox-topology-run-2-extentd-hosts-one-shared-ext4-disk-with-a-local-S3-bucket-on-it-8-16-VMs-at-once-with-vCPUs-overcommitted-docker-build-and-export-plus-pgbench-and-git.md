---
id: TASK-330.1
title: >-
  Sandbox topology run: 2 extentd hosts, one shared ext4 disk with a local S3
  bucket on it, 8-16 VMs at once with vCPUs overcommitted, docker build and
  export plus pgbench and git
status: To Do
assignee: []
created_date: '2026-11-10 14:00'
labels:
  - testing
dependencies: []
parent_task_id: TASK-330
priority: high
ordinal: 730000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Builds on TASK-310's fxfs-guest-shared-disk case and TASK-311's fxfs-guest-docker-build-dax. Use an OSI-licensed S3 server (RustFS is Apache-2.0; check the licence before adding one). The run fails on any guest kernel report (TASK-311's first-oops capture), any VM let go, any slow framing over 1 s, or a sandbox command over its budget. Short mode under 10 min for every main push; long mode nightly.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The run exists, passes on main, and fails on 6a513509a..96c7808b8's write-behind regression put back (TASK-310) as a mutant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
