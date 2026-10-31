---
id: TASK-79.85
title: >-
  Branches in object storage that no handle names, made before TASK-79.78, are
  never reclaimed
status: Done
assignee: []
created_date: '2026-10-23 09:16'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: task
ordinal: 344000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.78 (round 2c) stops a failed fork from leaving a branch no handle names: a deletion that stops partway leaves the fork's handle, and deleting it lets the collector free what the fork inherited. Orphans made before it (a live record, no handle on any host) stay: the collector cannot tell a branch no store holds from one held idle on some host without a liveness protocol, so it keeps what they inherited for ever.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A way exists to find and delete branches no host holds (a liveness record, or an operator command that lists records against hosts' handles)
- [x] #2 Deleting one lets the collector free what it inherited, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxdax-gc URL --branches [--held FILE] lists the live branches but those FILE names (what hosts hold, by fxdaxctl stat's IDs); --delete-branch ID deletes one whichever placement took it last (token u64::MAX). branch::live is the listing. Test: branches_no_host_holds_keep_what_they_inherited_until_deleted (kept while live, listed, freed by the next pass once deleted); gc tests for the commands. extent_remote 24/24 in that filter on macOS.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax-gc --branches [--held FILE] lists live branches no host holds, --delete-branch deletes one whichever placement took it last. branches_no_host_holds_keep_what_they_inherited_until_deleted, gc tests. Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
