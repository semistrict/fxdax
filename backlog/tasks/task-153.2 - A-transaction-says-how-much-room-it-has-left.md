---
id: TASK-153.2
title: A transaction says how much room it has left
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 22:52'
updated_date: '2026-11-01 23:03'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 546000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Nothing owns how much a transaction can hold: every loop guesses with its own constant (MAX_ZEROED_PER_COMMIT, MAX_CLONED_*, WRITE_MANY_BYTES, BATCH_MUTATIONS). Explains TASK-139, the clone fragment bugs, generic/303 and the punch that spun. trim_some nearly has it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every such loop adds until the transaction says stop, its constant gone
- [ ] #2 The model test passes its hunts
- [ ] #3 generic/303 and the punch bug, put back, fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
