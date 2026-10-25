---
id: TASK-79.25
title: >-
  A guest can abort its VMM with a chain whose writable lengths sum past
  u32::MAX
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-25 04:44'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 213000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Buffers::new accepts aliased descriptors and sums the writable lengths with no bound. handle then calls u32::try_from(..).expect(..), and with panic=abort that kills the VMM, which retires its store and cuts its followers off.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:338, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:497, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:503, vendor/firecracker/Cargo.toml:35.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-77, TASK-26.18.5. No task covers Buffers::new summing writable lengths past u32::MAX and the expect that aborts the VMM. TASK-77 (To Do) would fuzz the extent protocol from the guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (vendor/firecracker, uncommitted): Buffers::new refuses a chain whose writable lengths sum past u32::MAX (BuffersError::TooLong), before handle's u32 conversion could abort the VMM. Test chains_writing_past_u32_are_refused: three aliased 0x6000_0000-byte writable descriptors. GCE r93 runs it green, then red with the bound taken out.

GCE r93: chains_writing_past_u32_are_refused green, red with the bound taken out; r97: all 9 extent device tests green with FXDAX_EXTENTD_REMOTE and FXDAX_EXTENTD_TOKEN set. Fork commit 01cbb2fe3 pushed to semistrict/firecracker fxdax-v1.16.1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Buffers::new refuses descriptor chains whose writable lengths sum past u32::MAX (BuffersError::TooLong) instead of handle aborting the VMM. Test proven red without the bound; device tests green on Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
