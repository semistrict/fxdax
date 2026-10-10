---
id: TASK-25.5
title: 'libkrun: set the guest''s clock precisely at boot'
status: To Do
assignee: []
created_date: '2026-10-10 19:08'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: bug
ordinal: 30000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Files on virtio-fs are stamped by the host's clock, which is ~170 ms ahead of a libkrun guest's on GCE (mtime-ahead-of-guest in docs/measurements/gce-fs-bench-2026-10-10T1823Z.md), while ext4 stamps them with the guest's own. make warns of clock skew, and a build's generated files can look out of date (GNU make's configure failed once, gce-fs-bench-2026-10-10T1731Z). The guest's clock starts from the RTC's whole seconds. Set it from the host's at boot, to the microsecond, in libkrun's init.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A file the guest writes on virtio-fs is stamped within 5 ms of the guest's clock, measured by the benchmark's mtime observation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
