---
id: TASK-149.38
title: 'pjdfstest on a FUSE mount of a store, in CI'
status: To Do
assignee: []
created_date: '2026-11-07 05:20'
labels:
  - testing
  - fuse
dependencies:
  - TASK-222
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 661000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
pjdfstest checks POSIX semantics (permissions, ownership, setgid, sticky directories, times, errnos) and runs against the guest's Fxfs, but only in the kernel and nightly suites, and the nightly has never run (OPEN_QUESTIONS: no GCP credentials), so in practice only when someone runs scripts/test-guest-kernel.sh; and only in directories it makes, never on a filesystem's root. A FUSE mount of a store (TASK-222) runs it on the host's Node layer and fxfs-posix in a CI job in minutes. pjdfstest is BSD-2-Clause; the guest's case pins https://github.com/pjd/pjdfstest at 85a8aea9e685999ef0540392fd80535f873d7ff7. It needs root for chown tests: CI's runner has sudo.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A CI job builds pjdfstest at the guest case's pinned commit and runs it as root on a FUSE mount of a new store, failing on any test failure but those listed as known, each with a task
- [ ] #2 A run also starts from the mount's root, chowned and chmodded first
- [ ] #3 Failures it finds are fixed or listed with tasks
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
