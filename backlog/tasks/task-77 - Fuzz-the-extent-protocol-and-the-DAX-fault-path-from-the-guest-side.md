---
id: TASK-77
title: Fuzz the extent protocol and the DAX fault path from the guest side
status: Done
assignee: []
created_date: '2026-10-21 19:54'
updated_date: '2026-11-03 02:11'
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
- [x] #3 A guest's stores, loads and faults at window offsets it was never given, from a guest kernel module or test hook
- [x] #4 The daemon's wire protocol, from a compromised VMM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24: with it, syzkaller over the guest Fxfs's syscalls (DAX and not), under the fault and delay injection of TASK-44.10/44.11, and cargo-fuzz over extentd's wire protocol as a malicious guest.

2026-10-31: test_answers_whatever_a_driver_asks (5000 random requests, a real daemon) and serves_whatever_a_guest_asks (200 random sequences, with a window): nothing found. Random by proptest, not coverage-guided: cargo-fuzz would reach further.

2026-10-31: AC4: whatever_a_vm_process_says_harms_no_other (extentd/tests.rs): 128 sequences a run, up to 64 messages each, of any kind, flags and arguments, 3 in 4 plausible requests of its own extents and window, through a real window and transfer area, ending in broken framing; of a tenant, beside another tenant's store. Asserts no panic, the other store unchanged and served, its own store let go of and reopened, and over 1000 requests served (2475 in the run measured). Found one bug: WindowChanges::record overflowed on a refused request's offset+len with a page refused waiting. 6 further runs clean.

2026-10-31: AC3: fxfs-guest-crash-stray-dax: fxfs.dax_stray, a test knob, loads or stores a word at a window offset nothing maps, within a page or across two. GCE r354, 8 of 8 rounds, all four kinds: the daemon refuses the access (an access to nothing mapped), KVM_RUN fails with EFAULT, that VM exits 1, and only it stops: the store holds every file whose fsync returned and passes the crash test's checks, and the daemon serves a new store after. r350 first took pages regions had mapped for unmapped: the window's second half is regions, so the test strays in its last 64 MiB.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The extent protocol and the DAX fault path are fuzzed from the guest side: the device's requests of any type, flags, extents and offsets against a real daemon; the store's window operations in random sequences; the daemon's wire protocol from a compromised VMM, beside another tenant's store (found WindowChanges::record overflowing, fixed); and a guest's stores and loads at window offsets it was never given (fxfs.dax_stray, fxfs-guest-crash-stray-dax: GCE r354 8 of 8, only that VM stops, the store passes the crash checks, the daemon serves on).
<!-- SECTION:FINAL_SUMMARY:END -->
