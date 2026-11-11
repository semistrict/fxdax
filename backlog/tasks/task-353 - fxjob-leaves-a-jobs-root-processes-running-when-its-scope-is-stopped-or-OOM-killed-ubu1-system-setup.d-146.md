---
id: TASK-353
title: >-
  fxjob leaves a job's root processes running when its scope is stopped or
  OOM-killed (ubu1 system-setup.d/146)
status: To Do
assignee: []
created_date: '2026-11-11 12:12'
labels:
  - bug
  - testing
dependencies: []
priority: medium
ordinal: 743000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxjob (ubu1:~/docs/system-setup.d/146-fxdax_cgroups.sh) runs a job in a scope of the user's systemd manager; sudo inside it stays in the scope, but the user's manager can't signal root's processes: when the kernel's OOM killer kills one process at the job's MemoryMax, systemd stops the scope with 'Failed to kill control group ... Operation not permitted', and the root processes run on (TASK-344's Linux suite, 2026-11-11: nextest and its test binaries, as root, after the scope failed 'oom-kill'). scripts/lib/host-remote.sh now reaps them itself (reap, by /proc/*/cgroup); other agents' fxjob users don't. Make fxjob itself kill what is left of its scope once systemd-run returns (sudo kill of the scope's cgroup.procs, as ramon may sudo), as a new version of step 146, rolled out so that fxjobs running as it is replaced aren't broken (write the new script beside and mv it into place).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A job whose sudo'd processes outlive its scope's end (OOM or stop) leaves none running after fxjob returns
- [ ] #2 Step 146 rewritten and run on ubu1 without breaking fxjobs running then
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
