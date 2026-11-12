---
id: TASK-378
title: >-
  ubu1: ordinary work is OOM-killed at 0.6-1.6 GB while a measurement holds its
  16 GB: user-1000.slice's MemoryMax 21G leaves all other work about 5 GB
status: To Do
assignee: []
created_date: '2026-11-12 10:27'
labels:
  - bug
  - ubu1
dependencies: []
priority: high
type: bug
ordinal: 760000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-12, step 146 (system-setup.d/146-fxdax_cgroups.sh, TASK-371's session's design, OPEN_QUESTIONS 'ubu1: work best effort, measurements isolated by the kernel'): user-1000.slice MemoryMax=21G, fxdax-bench.slice MemoryMin=16G. With TASK-330.4's run in fxdax-bench.slice at 16.3 GB, three fxjob work scopes were OOM-killed within the hour: t371-kernel (vmlinux link, peak 598 MB, with t364-fc killed in the same second), t371-fxdax-tests (cargo build, peak 939 MB), t371-ci-fix (cargo nextest -p fxdax, peak 1.6 GB, --mem 12G). free -g showed 15 GB available host-wide meanwhile: the cap is the user slice's, not the host's. So cargo builds of the workspace can't run on ubu1 while a measurement runs, which is most of the day. Options: raise user-1000's MemoryMax to what the host can give (27 GB total, ZFS ARC capped at 4G), or make it MemoryHigh, keeping the bench's MemoryMin; check what the 21G cap was protecting (the host's own services) before.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A cargo build of the fxdax workspace's tests under fxjob runs to completion while a bench job holds 16 GB
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
