---
id: TASK-367
title: >-
  host-remote.sh's clean races a cancel: two cleans of one run both rm -r it,
  and both fail
status: To Do
assignee: []
created_date: '2026-11-11 22:21'
updated_date: '2026-11-12 02:46'
labels:
  - scripts
  - ubu1
dependencies: []
priority: low
type: bug
ordinal: 751000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-11 (TASK-330.4): `scripts/bench-ab-host.sh cancel RUN` while that run's own driver still waited for it. The cancel ended the job; the waiting driver saw it end (host_wait), and both called host-remote.sh's clean at once: each `sudo rm -r` of the run's directory failed on files the other removed ('cannot remove .../job/log: No such file or directory'), each said 'Removing ... failed', and the run's directory and its worktrees (git worktree list) were left until a second cancel. Expected: clean is safe to run twice at once, the second a no-op (a lock on the run's directory, or rm -rf of what is left and git worktree prune after), and cancel says the driver's cleaning is its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Two cleans of one run at once leave no directory or worktree of it, and neither fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12: again, the second cancel of TASK-330.4's A/B while its nohup'd driver waited: both cleaned, both failed, the directory left until a third clean.
<!-- SECTION:NOTES:END -->
