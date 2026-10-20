---
id: TASK-50.1.1
title: 'Jail: a restored VM''s output lost to the runner (fxfs-guest-snapshot-dax)'
status: To Do
assignee: []
created_date: '2026-10-20 12:12'
labels: []
dependencies: []
parent_task_id: TASK-50.1
priority: high
type: bug
ordinal: 157000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Under the real seccomp filter in the jailer (scripts/test-e2e.sh FXDAX_FIRECRACKER_SECCOMP=real FXDAX_FIRECRACKER_JAIL=1, GCE run real4), fxfs-guest-snapshot-dax failed: 'expected "OK a\nOK b\n", got ""', though the test's stdout.txt holds 'OK a\nOK b' and both restored VMs printed and exited. No call was refused. Passed in the jailer under the log filter (jail7). Suspect: the runner's capture of the test's output while the restored Firecrackers, which the jailer's change of user makes forget PR_SET_PDEATHSIG, or the jail's mount namespace, hold the test's pipes or files.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Reproduced by looping the case in the jailer (FXDAX_E2E_ROUNDS)
- [ ] #2 Cause found and fixed; the case passes 20 rounds in a row in the jailer under the real filter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
