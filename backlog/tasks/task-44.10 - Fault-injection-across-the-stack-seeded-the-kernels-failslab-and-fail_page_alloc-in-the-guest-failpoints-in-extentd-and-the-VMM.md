---
id: TASK-44.10
title: >-
  Fault injection across the stack, seeded: the kernel's failslab and
  fail_page_alloc in the guest, failpoints in extentd and the VMM
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 389000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-99 hid behind a 4 KiB kmalloc that fails only under memory pressure; fxfs.dax_fail_held found it by forcing that one path. Allocation failures, MAP and WRITE errors, slow or dropped replies are paths no normal run takes. The kernel's fault-injection framework covers every allocation at a set probability; the fail crate (Apache-2.0) does the same for named points in Rust.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Guest suites can run with failslab and fail_page_alloc at a probability, seeded, the seed in the results
- [ ] #2 extentd and the VMM have failpoints at their protocol's steps (MAP, UNMAP, WRITE, COMMIT replies), enabled by environment, seeded
- [ ] #3 A nightly or on-demand GCE run with injection on, its findings filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
