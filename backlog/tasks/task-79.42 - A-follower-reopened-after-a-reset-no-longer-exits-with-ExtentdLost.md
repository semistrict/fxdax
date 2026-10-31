---
id: TASK-79.42
title: A follower reopened after a reset no longer exits with ExtentdLost
status: Done
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 230000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
reopen() never registers on_daemon_lost on its new store, so when the daemon goes, the follower's requests fail with EIO and its next window fault SIGABRTs the VMM instead of exiting with status 158.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:204, vendor/firecracker/src/vmm/src/devices/virtio/extent/device.rs:331, crates/fxdax/src/extentd/client.rs:647, crates/fxdax/src/dax/uffd.rs:658.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-50.2, TASK-42.3, TASK-70, TASK-56. TASK-50.2 (Done) added on_daemon_lost and ExtentdLost (exit 158). TASK-42.3 step 6 added 'a reset follower reopens its store at activation'. No task covers reopen() missing the registration.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. reopen registers on_daemon_lost (stop_with_daemon, shared with make). Proved only by fxfs-guest-follow-remount's last step, NOT YET RUN.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A reopened store stops the VM with its daemon, as the first did. The follower's exit 158 ends fxfs-guest-follow-remount; without the hook, 'the follower's VM runs still 10s after its daemon was killed' (r302). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
