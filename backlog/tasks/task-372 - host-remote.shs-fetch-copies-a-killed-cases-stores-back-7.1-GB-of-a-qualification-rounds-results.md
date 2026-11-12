---
id: TASK-372
title: >-
  host-remote.sh's fetch copies a killed case's stores back: 7.1 GB of a
  qualification round's results
status: To Do
assignee: []
created_date: '2026-11-12 04:37'
labels:
  - scripts
  - ubu1
dependencies: []
priority: low
type: bug
ordinal: 755000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-12 (TASK-330.4): a qualification A/B whose rounds job the kernel OOM-killed (systemd stopping its scope) never ran qualify.sh's prune, and host_fetch tarred the results whole: the sandboxes case's stores, bases and images (*.root, *.data, *.base, sandbox-image, store-*), 7.1 GB copied to the driver's machine. fetch excludes rootfs, *rootfs.ext4, jail and *-cache only. Expected: what a host run fetches are results, whatever the job left: fetch leaves out stores and images by the names the cases give them, as qualify.sh and bench-ab-linux-host.sh prune them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A host run killed mid-case fetches its logs and measurements but no store or image
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
