---
id: TASK-79.2
title: >-
  Guest-chosen extent and segment sizes end up as whole-object host buffers: one
  guest can OOM extentd or fill the host disk
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 190000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
WRITE's end is not bounded by max_extent on the store path, and offset+len is added unchecked (it wraps in release builds). Two 4 KiB writes make a 150 GiB extent, which seal and the uploader then allocate and zero-fill whole. Rewrites of an open extent append to the open segment without being charged to capacity, segment size is checked only at commit, and extentd never cleans, so the host's files grow without limit. gather reads each closed segment whole, keeps about three copies, and PUTs it as one object, which over 5 GiB is retried forever.

Where: crates/fxdax/src/extents.rs:1085, crates/extent_store/src/store.rs:1256, crates/extent_store/src/store.rs:1390, crates/extent_store/src/store.rs:1420, crates/extent_store/src/store.rs:1886, crates/extent_store/src/store.rs:2056, crates/extent_store/src/store.rs:2402, crates/fxdax/src/remote.rs:791, crates/fxdax/src/remote.rs:813, crates/extent_remote/src/writer.rs:366, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:529.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-50.3, TASK-26.10, TASK-28.5, TASK-77. No task covers unbounded guest-chosen extent or segment sizes, the unchecked offset+len, uncharged appends, or extentd never cleaning. TASK-50.3 (Done) caps the kernel's page memory only. TASK-26.10 (Done) built store cleaning. TASK-77 (To Do) is generic guest-side fuzzing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Finalization check (after the push of 47b7a348): not done.  Audit: AC#2 ('Fixed') is not proven for the 'fill the host disk' half of this bug. f5800c11 replaced the room bound on sealed dead space, and both its message and state.rs:206 on main say the host disk across commits is 'bounded by cleaning's pace alone'. TASK-79.89 (To Do, High, filed by 56cfdee9 as this task's leftover) says a guest that frees faster than cleaning, or whose segments stay just above CLEAN_BELOW live, grows host disk without limit and needs a design decision. The r3 reviewer called this reversing the task's High-severity guarantee and said the user should decide; the item stayed 'blocked' in r3-result.json. So this task's failure ('one guest can ... fill the host disk', 'the host's files grow without limit') can still happen through another access pattern. cleaning_between_commits_bounds_what_segments_hold bounds one pattern only, with clean_behind called between commits. The rest is proven: the OOM half (a81f8636 max_extent and checked wrap, 142d5140 segment_limit) has before-failures, and every named test, the extentd cleaning tests (Linux 699 and 700) and all five guest tests (no 'found the store full') pass. AC#1 and DoD#1-3 hold, with the same Linux caveat as TASK-79.1; the failing extentd test was made more likely by this task's 9c95cf12. This can close once the user accepts TASK-79.89 as owning the bound across commits.
<!-- SECTION:NOTES:END -->
