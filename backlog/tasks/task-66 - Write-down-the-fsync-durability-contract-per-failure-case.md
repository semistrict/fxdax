---
id: TASK-66
title: Write down the fsync durability contract per failure case
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-31 13:52'
labels: []
dependencies: []
priority: medium
ordinal: 175000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest fsync is an ordered COMMIT, durable within 50 ms and uploaded within about 1 s (doc-2). Write down what an acknowledged fsync survives: the VM dying, extentd dying, a host reboot, host loss.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: doc-10 says it, failure by failure, with what tests each row. Reading the store for it found an acknowledged fsync is not kept across extentd being killed: a commit's record is in extentd's memory until the flush, up to 50 ms.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
doc-10. Three rows have no test of what they keep: TASK-67.
<!-- SECTION:FINAL_SUMMARY:END -->
