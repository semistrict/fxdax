---
id: TASK-159
title: The control socket follows the umask
status: Done
assignee: []
created_date: '2026-11-02 02:53'
updated_date: '2026-11-04 10:39'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 566000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-155: the default socket is 0600 under a umask; the control socket still follows the process umask, so under umask 000 it is world-connectable, and it can do everything. It was left as a VM manager might rely on group access.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 It is 0600 or a chosen group, with no window, and a test shows it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The control socket is 0600, the daemon's user's, whatever the umask, and a tenant's socket 0660 from the moment it appears at its path: bind_given binds in a private directory and links the socket into place. Tests the_control_socket_refuses_other_users, tenants_sockets_are_of_their_mode_from_the_first, sockets_are_not_bound_over_another. Decision in OPEN_QUESTIONS.md; the integrator guide updated. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
