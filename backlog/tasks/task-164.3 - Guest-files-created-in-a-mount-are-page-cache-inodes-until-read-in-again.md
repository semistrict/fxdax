---
id: TASK-164.3
title: 'Guest: files created in a mount are page-cache inodes until read in again'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-04 16:35'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 613000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
S_DAX set only at read-in (doc-13).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A created file read in after eviction is DAX unless written since the last seal began
- [ ] #2 Mixed inodes in one superblock pass the DAX and no-DAX suites
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Files created in a mount are page-cache inodes until read in again after eviction (S_DAX set at read-in only), unless written since the last seal began; cow_created_in_page_cache passes. Verified: macOS scripts/test.sh (workspace 2,089/2,089, Shuttle, scripts); GitHub CI, every job (run 41859737932: Linux workspace, Shuttle incl. the uffd private-memory models, guest-sim's 3,000 seeds and mutants); GCE r546 copy-on-write off in the jailer under the production seccomp filter (5 suites, refusal gate clean), r547 copy-on-write on (crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass).
<!-- SECTION:FINAL_SUMMARY:END -->
