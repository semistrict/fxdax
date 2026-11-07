---
id: TASK-235
title: >-
  test-gce.sh takes a job whose starting SSH dropped after the launch for one
  that never started, and fetches its results before it ran
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 18:37'
updated_date: '2026-11-07 20:24'
labels: []
dependencies: []
references:
  - scripts/lib/gce.sh
  - scripts/tests/test_gce.py
priority: medium
type: bug
ordinal: 673000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found on 2026-11-07 (TASK-229's r942, on fxdax-test-r942-b in us-west1-b). gce_remote_detached starts a job over SSH (setsid nohup, ~/RUN.job) and returns that SSH's status if it fails, so that a driver doesn't wait until the host's end for a job that never started (test_a_job_that_did_not_start_is_not_waited_for).

But the starting SSH can fail after the job launched: r942's connection timed out ("Read from remote host ...: Operation timed out", exit 255) once the job was running. test-gce.sh then fetched the results at once, every case unrun, and reported the run over while it went on for an hour on the host. Waiting needed a manual scripts/test-gce.sh resume with the run's name, found by SSH.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 After a failed starting SSH, gce_remote_detached checks over a fresh connection whether the job launched (its pid, written as it is launched), waits for it if so, and returns the starting SSH's status if not
- [x] #2 test_a_job_that_did_not_start_is_not_waited_for still passes: a job that didn't start is not waited for
- [x] #3 A test of a start whose connection drops after the launch: the job is waited for and its status returned
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Write the job's pid as it is launched (~/RUN.job/pid).
2. After a failed starting SSH, look for the pid over a fresh connection: wait if found, else return the start's status.
3. fake_gcloud: each SSH's status and output in turn (ssh_calls); a test of a start dropped after its launch, red first; the not-started test kept.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
scripts/tests: test_gce.py 37 tests pass, every script test passes (191) (python3 -m unittest discover -s scripts/tests). Red without the fix: a_job_whose_starting_connection_dropped_is_waited_for returned 255 with nothing waited for. a_job_that_did_not_start_is_not_waited_for passes, its calls one SSH more (the look for the pid), still 255 and no wait. No GCE run needed (the user, 2026-11-07).

Correction, before landing: r943 (fxdax-test-r942-b, this branch's first version) left no pid file. The start was `mkdir && rm -f status pid && setsid ... & echo $! > pid`, where `&` backgrounded the whole list, so the pid was written before its directory existed or removed by the rm after. The launch and its pid are now grouped alone in the background: `mkdir && rm && { setsid ... & echo $! > pid; }`. The fake-only tests couldn't see it, as the fake ran no command. fake_gcloud now runs an SSH's command in a local shell (ssh_run), and two tests run the start for real: a_job_keeps_its_pid_and_is_waited_for and a_job_whose_starting_connection_dropped_is_waited_for (red ungrouped: no pid, status 1, and 255). 37 test_gce tests and all 191 script tests pass.
<!-- SECTION:NOTES:END -->
