---
id: TASK-60.1
title: >-
  Guest Fxfs's Rust allocations are infallible: one failing panics the kernel,
  or, in linked-hash-map's guard node, writes through NULL
status: To Do
assignee: []
created_date: '2026-10-29 21:11'
updated_date: '2026-10-30 01:27'
labels:
  - fxfs
  - bug
dependencies: []
parent_task_id: TASK-60
priority: medium
ordinal: 470000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r260 (fxfs-guest-faults-dax on a debug kernel failing the writers' slab and page allocations at 1%, failslab and fail_page_alloc): 'BUG: kernel NULL pointer dereference, address: 00000000000000a0' in linked_hash_map's LinkedHashMap (Fxfs's object_store::tree_cache shard) applying a mutation, from fxfs_create's transaction commit. linked-hash-map 0.5.6's ensure_guard_node calls std::alloc::alloc and writes through it unchecked; fxfs_rt_alloc (guest/linux/fs/fxfs/rt.c) is kvmalloc(GFP_KERNEL), which may return NULL. Every other Rust allocation, failing, ends in handle_alloc_error, a panic, which fxfs_rt_panic makes the kernel's. In practice the kernel retries small GFP_KERNEL allocations ('too small to fail'), unless the task is the OOM killer's victim, or fault injection fails them. Options: __GFP_NOFAIL for small Rust allocations (the kernel's own answer for allocations that can't fail), or fallible allocation where Fxfs can fail the operation (TASK-60), and linked-hash-map patched or replaced either way (its guard node, at least, unchecked).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A Rust allocation failing in the guest neither dereferences NULL nor panics the kernel
- [x] #2 fxfs-guest-faults-dax with Rust's allocations failing too passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r262 (the same test, fxfs_rt_alloc's allocations exempt): 'Kernel panic - not syncing: fxfs: panicked at linux-rt/src/executor.rs:134:5: assertion failed: no memory to schedule a task (-12)': the executor's fxfs_rt_schedule (rt.c, its work item's allocation) failing, which the executor asserts can't, as losing the poll would leave the task waiting for ever. Its parker and timer allocations (fxfs_rt_parker_new, fxfs_rt_timer_new) are the same. The runtime's C allocations need __GFP_NOFAIL, or room set aside, as much as the Rust allocator's.

Fixed (b3f83cfae, on main): fxfs_rt_alloc, the parker's and the timer's allocations __GFP_NOFAIL; each task's work item (fxfs_rt_work) made with the task, from async_task's metadata, so a waker queues a poll allocating nothing (fxfs_rt_work_queue), the work freed with the task, its run reading nothing of it after calling into Rust. fxfs-guest-faults-dax no longer exempts the runtime from the injection (its allocations NOFAIL, which failslab and fail_page_alloc skip). GCE r261, debug kernel (KCSAN, lockdep, fault injection): fxfs-guest-faults-dax, -checks-dax, -dax 3 rounds each passed; KCSAN's reports the same set as before the change (r255 at 7c9948612), the kernel's own.
<!-- SECTION:NOTES:END -->
