---
id: TASK-79.101
title: >-
  extentd's watchdog cannot tell a VM process from a child that inherited its
  socket, on kernels without pidfd for it
status: Done
assignee: []
created_date: '2026-10-24 03:39'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 372000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.9 (round 2b). The watchdog that lets go of unanswering VM processes kills and waits by pidfd. A VM process's forked children that keep the socket are not tracked, and on kernels before 6.5 a VM process that exits leaving the socket to a child leaves a pid-reuse race (documented in peer_process's doc).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The watchdog acts on whatever process holds the socket's other end, or refuses a socket shared with children
- [x] #2 A test forks a child holding the socket and checks the watchdog's outcome
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
A connection now ends with its process's exit (select on the pidfd in the connection's task), whoever holds the socket. Test (real fork): vm_processes_that_leave_their_sockets_to_children_are_gone; fails without the fix (the store stays EBUSY), Docker Linux. peer_process's pre-6.5 race, a pid reused between exit and pidfd_open with a child keeping the socket, stays documented: nothing in SO_PEERCRED tells the two apart.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A connection ends with its process, whoever holds the socket: the child's end is shut, nothing is signalled. vm_processes_that_leave_their_sockets_to_children_are_gone (fails without, Docker; passes on r292). Left, documented on peer_process: before Linux 6.5, a pid reused between an exit and pidfd_open while a child keeps the socket. Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
