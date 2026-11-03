---
id: TASK-160.32
title: 'One set of DAX mutants, each naming its killer'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 603000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/guest-mutants-gce.py (Python string edits) and guest/sim/mutants (patches) duplicate at least 4 bugs; the gate spends hours on GCE mutants the sim finds in seconds; neither records which check should kill a mutant. Proposal: one directory of patches with a header naming the expected killer and kernel; the gate runs the sim first and sends to GCE only what needs a real kernel; killed only when the named killer fails. After the CI and results tasks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One mutant set
- [ ] #2 Each mutant's fate is its named killer
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
