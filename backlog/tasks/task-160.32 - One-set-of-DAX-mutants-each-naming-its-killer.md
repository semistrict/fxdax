---
id: TASK-160.32
title: 'One set of DAX mutants, each naming its killer'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 03:06'
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
scripts/guest-mutants-gce.py (Python string edits) and guest/sim/mutants (patches) are two mutant sets; the gate has spent hours on GCE mutants the sim finds in seconds; neither records which check should kill a mutant. Proposal: one directory of patches with a header naming the expected killer and kernel; the gate runs the sim first and sends to GCE only what needs a real kernel; killed only when the named killer fails. After the CI and results tasks.

At 9f81f6018, writable DAX's deletion (37c331a6c) took eleven of guest-mutants-gce.py's mutants (the seals rechecks, seal_keeps_writable, held_not_written, the moved_* and moves_before_bytes, map_failure_unretried) and most of guest/sim/mutants' writable-DAX ones. Left: six GCE mutants (map_to_unlocked, resize_moves_the_last_block, regions_find_unlocked, inherited_acls_written_back, fault_ignores_fatal_signals, store_fails_on_pins; scripts/guest-mutants-gce.py:41-) and 28 sim patches. One bug is in both: map_to_unlocked and guest/sim/mutants/task-44.19-map-to-unlocked.patch, which, copying on write, neither the sim's oracles nor any guest suite kills (guest/sim/README.md:187, guest-mutants-gce.py:42-43). The headers' killers would say so.
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
