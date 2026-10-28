---
id: TASK-112.3
title: >-
  Design: whether a guest fsync needs a seal, and whether the seal's window MAPs
  can leave the commit path
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 02:30'
updated_date: '2026-10-28 03:25'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: medium
ordinal: 426000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox round 7b (TASK-112): DAX seals cost 0.54 ms against 0.32 without, about three window MAPs each (1.8 evacuation, ~1 restage) at ~90 us. They ask: could fsync commit an open extent durably without write-protecting and evacuating (sealing only at fork, snapshot or window reclaim); or could evacuation and restage run after the commit replies. Related: TASK-66 (fsync durability contract), TASK-109.1 (evacuation remaps), TASK-109.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The answer to both questions, with the durability and sharing constraints behind it, is recorded in a decision or doc
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
doc-9 answers the round 7b questions with this run's numbers. Every fsync must seal: write-once extents are what durability by whole extents, sharing by ID and the shared page cache rest on; a durable-but-open staging extent is possible but costs store-format, protection and seal-on-share changes. The seal's restage and evacuation MAPs are already off the guest's commit path (fxfs_dax_sealed queues them). On the commit path DAX adds ~100 us in extentd (58 sealing, 44 writing the data stored through the window) and ~66 us of window changes before the reply: 430 us against 263. DAX's lower tps is mostly off the commit path: evacuation holds each file's DAX semaphore ~650 us with a MAP inside, 32 s a run (TASK-112.4), and stores wait 4.9 s a run for seals. No-DAX's ceiling is the nested round trip (~238 us of 263 outside extentd).
<!-- SECTION:FINAL_SUMMARY:END -->
