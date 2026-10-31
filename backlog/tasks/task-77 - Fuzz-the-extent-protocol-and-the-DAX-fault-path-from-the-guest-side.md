---
id: TASK-77
title: Fuzz the extent protocol and the DAX fault path from the guest side
status: In Progress
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-10-31 14:06'
labels: []
dependencies: []
priority: high
ordinal: 186000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Concurrency is covered by Shuttle and TLA+; nothing fuzzes the guest-facing protocol. Fuzz the extent protocol and the DAX fault path from the guest side.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The device's requests: any type, flags, extents and offsets, against a real daemon: answered, within their room, and served on after
- [x] #2 The store's window operations in random sequences: no panic, and an extent they can't name unchanged
- [ ] #3 A guest's stores, loads and faults at window offsets it was never given, from a guest kernel module or test hook
- [ ] #4 The daemon's wire protocol, from a compromised VMM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24: with it, syzkaller over the guest Fxfs's syscalls (DAX and not), under the fault and delay injection of TASK-44.10/44.11, and cargo-fuzz over extentd's wire protocol as a malicious guest.

2026-10-31: test_answers_whatever_a_driver_asks (5000 random requests, a real daemon) and serves_whatever_a_guest_asks (200 random sequences, with a window): nothing found. Random by proptest, not coverage-guided: cargo-fuzz would reach further.
<!-- SECTION:NOTES:END -->
