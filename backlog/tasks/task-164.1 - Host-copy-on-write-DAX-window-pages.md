---
id: TASK-164.1
title: 'Host: copy-on-write DAX window pages'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-04 16:35'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 611000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DAX_COW, private_max, MAP_ZEROS, PRIVATE in the extent device (doc-13 stage 1), offered beside today's writable mode.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store to a sealed or zero page gets a private copy at the same window address; MAP and UNMAP free it; eviction skips it
- [ ] #2 Past private_max refused (exit 159); KVM's async writes don't copy
- [ ] #3 doc-4 says so; unit, Shuttle and GCE tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The host offers copy-on-write DAX windows: a store to a sealed or zero page gets a private copy at the same address, held in private memory laid out like the window so adjacent private pages are one mapping and trims never touch them (the first design, a mapping per page, overran the VMM's mapping budget at 110,857 pages on GCE r544 and stalled vCPUs); private_max reserved against the tenant; MAP_ZEROS, PRIVATE; a driver's features page written again replaces it. Verified: macOS scripts/test.sh (workspace 2,089/2,089, Shuttle, scripts); GitHub CI, every job (run 41859737932: Linux workspace, Shuttle incl. the uffd private-memory models, guest-sim's 3,000 seeds and mutants); GCE r546 copy-on-write off in the jailer under the production seccomp filter (5 suites, refusal gate clean), r547 copy-on-write on (crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass).
<!-- SECTION:FINAL_SUMMARY:END -->
