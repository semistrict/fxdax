---
id: TASK-357
title: >-
  extentd hostile on a loaded ubu1: a commit unanswered within 60 s ('mallory's
  store didn't commit') and connections that never end within 240 s
status: To Do
assignee: []
created_date: '2026-11-11 14:39'
updated_date: '2026-11-11 15:16'
labels:
  - extentd
  - testing
dependencies: []
priority: low
type: bug
ordinal: 743000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Replaying Hunt run 42949975683's hostile seeds for TASK-343 on ubu1 (FXDAX_HOSTILE_CASES=1920, under fxjob --mem 8G, nice 10), on trees from TASK-92.41 on, while other agents' jobs loaded the host:
- seed 55 on tmpfs, case 811: "mallory's store didn't commit". The durable COMMIT's status was None: SharedAnswers::wait timed out after WAIT, 60 s, or the connection ended unanswered.
- seed 54 on ZFS, case 907: the same assertion.
- seed 55 on ZFS: "a connection of mallory's never ended: Timeout" (WAIT*4).
- seeds 54 and 55 on tmpfs with the TASK-343 fix: the same timeout, both at about 1420 s into their runs, in the same minute, while ubu1's load rose from about 4 to 35 (another agent's job at 11 GB beside a build).
The same seeds passed all 1920 cases elsewhere: seed 55 on tmpfs before TASK-92.41 (94f63b137) and with the fix when the host was quiet; seed 54 on tmpfs on main. Store flushes in these logs took up to 5.5 s (logs 3967 ms), and commits were durable 4.4 s after they were ordered. So these look like the host's stalls outrunning the test's fixed waits, not a lost answer. That is not shown: the log names neither the case's connection nor what the daemon was doing when the wait gave up.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 When the hostile test's commit wait or a connection's end times out, it captures the daemon's threads' stacks (TASK-44.13) and names the case, so a run says whether the daemon was stuck or slow
- [ ] #2 Either a cause in the daemon is found and fixed, with a regression test, or the waits are shown to be outrun only by the host's stalls and are made to wait on the daemon's progress rather than a fixed time
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11 (claude, TASK-341/345's Linux run on ubu1, load average 12-17): a_compromised_vm_process_harms_no_tenant_beside_it failed after 361 s at hostile.rs:1862, alice = Err(Timeout) in WAIT * 4; the same build alone passed in 14 s.
<!-- SECTION:NOTES:END -->
