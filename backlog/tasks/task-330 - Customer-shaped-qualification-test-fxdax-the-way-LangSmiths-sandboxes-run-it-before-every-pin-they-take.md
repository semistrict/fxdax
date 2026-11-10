---
id: TASK-330
title: >-
  Customer-shaped qualification: test fxdax the way LangSmith's sandboxes run
  it, before every pin they take
status: To Do
assignee: []
created_date: '2026-11-10 14:00'
labels:
  - testing
dependencies: []
priority: high
ordinal: 729000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
LangSmith's sandbox CI found four problems our suites didn't: TASK-310 (write-behind doubles disk writes; framing stalls for seconds on a shared disk), TASK-311 (guest oops then soft lockups in 8-vCPU docker builds), TASK-317 (extentd memory grows to 6 GB under an upload backlog) and TASK-314 (a failing bucket hangs commits silently).

Why we missed them:
1. Our runs have the shape we imagine, not theirs. Ours: one VM, a dedicated disk, a fast bucket in memory or on its own disk, an idle host. Theirs: 2 extentd hosts and many VMs on one disk, RustFS on that same disk, and 8 vCPUs per VM on an 8-CPU runner.
2. Perf work measures the metric being tuned, not the resources it costs. TASK-221 tuned durable_lag and declared done; nothing checked extentd's bytes written per guest byte, or its memory.
3. Nothing runs long against a slow or failing dependency. The simulation checks failure correctness; nothing checks resource use over time.
4. Workloads are filesystem-semantic (xfstests, fsx, pgbench), not what customers run (docker build and export, compilers, git).
5. No CPU overcommit (TASK-149.19 is filed and not done).
6. Signals were invisible even where it did happen: the first oops scrolled away, and bucket errors were dropped. Now fixed by TASK-311 and TASK-314.

The subtasks close each gap. The gate runs on ubu1 under fxjob, and must pass before a pin is handed to LangSmith.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every subtask done, and the qualification run is documented in the README's testing section
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
