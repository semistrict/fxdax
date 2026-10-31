---
id: TASK-79.100
title: >-
  A VMM that closes its extentd socket while still alive has its pages released
  at the close
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
ordinal: 371000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.9 (round 2b). extentd releases a VM process's quarantined and parked pages when its connection closes, but a VMM that closes its socket while still running may still have them mapped for its guest, so they could be reused while the guest reaches them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Pages a VM process may still map are kept until that process is known to have exited
- [x] #2 A test closes the socket of a live VM process and checks its pages are not reused
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
A connection gone whose window maps pages, or with a taking operation unacknowledged, waits for its process: EXITING (5 s), as one exiting closes its socket just before it shows, then lets it go, killing it, and goes on once it has exited (Peer::gone). Store and pages wait for that. No pages reached: nothing waits, as a follower's reset needs. Test (real child process): vm_processes_that_close_their_connections_keep_their_pages_until_gone; fails without the fix ('the page the living VM process maps is free'), Docker Linux.

GCE r292 found it in the tree: a DAX follower's reset dropped its store with its window mapped (the guest leaves that to the device's going), so extentd held the store its 5 s, then killed the follower: fxfs-guest-follow-remount-dax. A real case of this task, through 79.24's store drop: the VMM's pages stayed mapped after the store went. reset_confined now unmaps the whole window first, acknowledged by the reply. r293 reruns the follower cases 3 rounds. r292 otherwise: Linux workspace 1986/1986, Shuttle suites, 18 of 19 guest cases.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A connection gone from a live process reaching pages waits EXITING, then kills it, pages and store waiting for its exit; a follower's reset unmaps its window first, as r292 found. vm_processes_that_close_their_connections_keep_their_pages_until_gone (fails without, Docker; passes on r292), follow-remount-dax 3 rounds r293. Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
