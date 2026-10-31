---
id: TASK-67
title: Crash soak rounds that kill extentd and drop uploads before host loss
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-31 13:52'
labels: []
dependencies: []
priority: high
ordinal: 176000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The 1,002-round soak kills only the VM. Add rounds that kill extentd, and that drop uploads before host loss, each checked against the durability contract.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: doc-10 names what this must show: what extentd's death keeps, what a host reboot keeps, what a lost host keeps, with a guest.
<!-- SECTION:NOTES:END -->
