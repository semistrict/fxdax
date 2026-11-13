---
id: TASK-376
title: >-
  fxjob ps and fxjob-isolate status print cat errors when no bench job runs: the
  cpuset files are gone from fxdax-bench.slice, and they should say isolation is
  off
status: To Do
assignee: []
created_date: 2026-11-12 09:39
updated_date: '2026-11-13T08:58:00Z'
labels:
  - bug
  - ubu1
dependencies: []
priority: low
type: bug
ordinal: 758000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-12, step 146 (system-setup.d/146-fxdax_cgroups.sh): with no fxjob --bench job, systemd leaves cpuset out of fxdax.slice's cgroup.subtree_control, so fxdax-bench.slice has no cpuset.cpus.effective nor cpuset.cpus.partition. fxjob ps prints 'cat: .../cpuset.cpus.effective: No such file or directory' and an empty cpus=, and fxjob-isolate status exits 1 with the same error for cpuset.cpus.partition. Isolation itself is fine: it is set up when a bench job starts. Only the reporting reads files that may be absent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxjob ps and fxjob-isolate status, with no bench job, print isolation off and exit 0, no error
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-381 (duplicate) adds the cause: after a manager reload, fxdax.slice's subtree has no cpuset controller until a measurement runs, so the cpuset files fxjob ps and fxjob-isolate status read are missing.

TASK-396: fxjob ps (scripts/fxjob) no longer errors: it reads a missing cpuset.cpus.effective from the nearest parent, and reads isolation status itself instead of sudo fxjob-isolate status. fxjob-isolate status itself (step 146, ~/docs) still reads cpuset.cpus.partition unguarded.
<!-- SECTION:NOTES:END -->
