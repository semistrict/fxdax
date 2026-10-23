---
id: TASK-79.70
title: >-
  Importing a file of about 1 GiB with the kernel's default 1 GiB of memory
  never finishes
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:14'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 321000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while measuring TASK-92.36 (round 1). `cargo run --release -p fxdax --example import-bench 16 1024` (one 1 GiB file, Config::default, which fxdaxctl import uses) was still writing after 10 min before TASK-92.36's change and after 1 min after it, with CPU down to about 12%; 768 MiB takes 0.17 s. Samples show the port receiver handling a steady stream of DIRTY requests (mark_dirty -> report_pager_dirty -> dirty_pages) while the write waits, so the stall is under memory pressure in the Zircon/Fxfs pager or dirty-bytes path, not in the importer. Samples were saved in the round's scratchpad (sample-1g*.txt) but are not durable; reproduce with the bench.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 import-bench imports a 1 GiB file with Config::default's memory in a time proportional to its size
- [ ] #2 A test or bench run in the repo pins it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
