---
id: TASK-155
title: A jailed VM process can not reach the default socket
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 556000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A jail reaches only what is bind-mounted into its mount namespace, so the default socket is out of reach as long as the VM manager mounts only the tenant socket. Make that hold at the kernel, not by convention: the default socket owned by the daemon, mode 0600, so that a jail uid can not connect even if mounted.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The default socket refuses a process of another uid
- [x] #2 A test shows it
- [x] #3 doc says what the VM manager must do
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bind the default socket under umask 0177 in the daemon, before any thread makes files: 0600 from the first, no window, whatever the daemon's umask.
2. FXDAX_FIRECRACKER_JAIL relies on reaching it (umask 000): the harness gives the socket to the jail's user after the daemon starts, its daemons' too.
3. Test: the real daemon binary under umask 0, a process of another uid refused through a bind mount, a socket beside it connectable as the positive control.
4. Docs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
bind_owner_only binds under umask 0177: 0600 from the moment the socket exists, no window, whatever the daemon's umask. Default socket users: fxdaxctl, tests and scripts use the control socket or the daemon's own user, but FXDAX_FIRECRACKER_JAIL relied on it, Firecracker as uid 64000 under umask 000. Harness fixed: scripts/test-e2e.sh and spawn_daemon give the socket to the jail's user. Looked around: the control socket's mode still follows the umask, and is world-connectable under umask 000, as that same jail mode has it; left, as a VM manager of another group may rely on it: raised with the user. Test the_default_socket_refuses_other_users: the real daemon under umask 0, bind-mounted, uid 64000 refused EACCES, a socket beside it connecting as the positive control; red with the umask taken out.

Verified at ef3dfc028, as TASK-154's notes say. the_default_socket_refuses_other_users passes on Docker and GCE (r483), red with the umask taken out, its child connecting. Each jailed guest case of r484 reached the socket given to the jail's user, FXDAX_FIRECRACKER_JAIL's harness change. DoD 1 is left open for the user, as on TASK-154.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The default socket refuses a process of another uid (made under a umask that leaves it the owner's), and the VM manager's duty to give the jail's user its socket is documented. the_default_socket_refuses_other_users passes on Docker and GCE (r483), red with the umask taken out; each jailed guest case of r484 reached the socket given to the jail's user (scripts/test-e2e.sh, spawn_daemon). Left as it was: the control socket's mode, which a VM manager of another group may rely on.
<!-- SECTION:FINAL_SUMMARY:END -->
