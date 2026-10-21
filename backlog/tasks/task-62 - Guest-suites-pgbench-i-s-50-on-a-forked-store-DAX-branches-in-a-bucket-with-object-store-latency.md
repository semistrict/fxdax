---
id: TASK-62
title: >-
  Guest suites: pgbench -i -s 50 on a forked store, DAX, branches in a bucket
  with object-store latency
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
labels: []
dependencies: []
priority: high
ordinal: 171000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Add the sandbox team's failing workload to the guest tests so it can't regress: import postgres:17, fork, pgbench -i -s 50 with parallel index build, DAX window 2 GiB, extentd branches in a bucket with injected latency (e.g. netem or a delaying object store), checking for completion and correct data.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
