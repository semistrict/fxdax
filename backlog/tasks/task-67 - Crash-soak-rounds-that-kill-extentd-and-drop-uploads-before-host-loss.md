---
id: TASK-67
title: Crash soak rounds that kill extentd and drop uploads before host loss
status: To Do
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-31 22:17'
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

2026-10-31: fxfs-guest-crash-dax-daemon-killed: extentd killed with the crash test's writers going: GCE r342: 1 of 296 acknowledged files went, and what stayed was a prefix, every writer's. The host reboot and the lost host rows of doc-10 are left.

2026-10-31: doc-10 in these notes is the durability contract, doc-12 now.
<!-- SECTION:NOTES:END -->
