---
id: TASK-39
title: Integrate fxdax into the main sandbox system
status: Done
assignee: []
created_date: '2026-10-17 19:16'
updated_date: '2026-10-18 02:45'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 110000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
What the main system (zygote restore, suspend-to-RAM, live migration, memory snapshots, tenant isolation, snapshot deletion, base images from Docker, our 6.1 kernel and Firecracker pin, placement fencing) needs of fxdax, from the integration review of 2026-10-17. Each subtask is one gap.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The subtasks are done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Order, as the user asked on 2026-10-17: hardest first: 39.6 (6.1 kernel backport and Firecracker series), 39.1 (snapshots), 39.2 (isolation), 39.3 (live forks), 39.5 (Docker import), 39.4 (delete a base with forks), 39.7 (fencing token, wait for uploads).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
All seven subtasks done and pushed (c18575a..afd84e4). Final x86-64 GCE run (576c6d7): workspace 2019/2019, guest suites 17/18 incl. pjdfstest (both), import, live fork, snapshot/suspend/migrate; the one failure is fxfs-guest-crash-dax, the intermittent pre-existing TASK-26.6.2 (evidence added there). Review fixes 8613c65 (idempotent deletes, cross-process store lock, host fencing token) verified on macOS 1996 / Linux 2023 / 16 Lima guest suites.
<!-- SECTION:FINAL_SUMMARY:END -->
