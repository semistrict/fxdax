---
id: TASK-330.5
title: >-
  Sandboxes run's long mode doesn't fit a GitHub standard runner: the runner
  goes down 6-66 minutes in
status: To Do
assignee: []
created_date: '2026-11-11 07:25'
labels:
  - testing
  - ci
dependencies: []
parent_task_id: TASK-330
priority: medium
ordinal: 741000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-330.1's .github/workflows/sandboxes.yml runs scripts/sandboxes.sh on GitHub's ubuntu-24.04 runner (4 vCPUs, 16 GB, 3 GB swap). The short mode (8 VMs at once, ~3-5 min) runs whole there. The long mode never has: 16 VMs at once, 10.75 and then 9 GiB of guests: 'The runner has received a shutdown signal' 7-7.5 min in (vmstat: free under 200 MB, swap 3 GB full); 8 at once of 32 with daemons of 4 GiB: down 6 min in, the RSS log showing each daemon's extent cache climbing to 2.9 GB beside 2.2 GB builders; daemons of 2 GiB: ran whole (12.5 min) but DAX windows refused (ENOMEM, no room to pin their private pages); daemons of 3 GiB, windows halved (main's sizes now): the job ended 66 min in with its run step never finished and no log uploaded (run 42944951179). So the nightly doesn't call it (nightly.yml unchanged). Options: a larger runner (GitHub's 8- or 16-core, paid), a self-hosted one, or a long mode sized for 16 GB (fewer at once, daemons' caches bounded lower with fewer windows), measured with the workflow's vmstat and RSS log. Done when the nightly runs the long mode whole.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
