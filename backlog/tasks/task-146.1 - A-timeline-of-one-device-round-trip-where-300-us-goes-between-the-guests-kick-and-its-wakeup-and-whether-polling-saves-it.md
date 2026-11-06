---
id: TASK-146.1
title: >-
  A timeline of one device round trip: where 300+ us goes between the guest's
  kick and its wakeup, and whether polling saves it
status: To Do
assignee: []
created_date: '2026-10-31 22:15'
updated_date: '2026-11-06 06:51'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: low
ordinal: 497000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One MAP and one READ timed at each hop, guest, VMM worker, daemon, stage operations, ack, completion
- [ ] #2 virtio_extent.spin_ns tried at 100, 600 and 1000 us on pgbench-dax, tps and CPU reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: virtio_extent.spin_ns=1000000 against the default, pgbench-dax, 5 runs, two hosts of one type: r358 1244/1133/1109/1109/1125 (mean 1144) against r362 1160/1063/1003/985/1031 (mean 1049): +9%. MAP request 235 us against 358; polled 973018 against 17 of about 1.1M. Guest CPU: system 41810 against 30973 ticks, idle 21759 against 31253. Not a default yet: it needs one host's A/B (AGENTS.md), and a VM spinning takes host CPU others may want: density is what the sandbox team bought DAX for.

2026-11-02, one host (us-central1-a, n2-standard-4), 106b6128d, pgbench-dax, interleaved A/B/A/B, 2 rounds each, 20 pgbench runs a side: virtio_extent.spin_ns=1000000 is slower: 789 against 828 tps (-4.7%), every run pair agreeing. It does shorten the round trip: MAP 498 against 627 us, write(2) 489 against 673 us, seal waits 12.4 against 37.4 s, polled 59% of completions against none. But guest system CPU rises 39% (112,700 against 81,090 ticks) and idle falls 22%: the spinning takes the CPUs pgbench's backends need. Results: /tmp/fxdax-t18-spin/{a1,b1,a2,b2}. Not a default. On the default side, the round trips are about 600 us: device writes 587, commits 585, MAPs 627, reads 807. Per transaction: 0.62 commits, 0.64 writes, 1.42 MAP requests. Over one VM: 390,465 MAP requests at 688 us (269 s), of which restage maps 151,335, nearly one per seal (155,008), and catch-ups 167,726; 98% of write(2)s go through. write(2)'s begin_other, 388 of 673 us, is not broken down: it includes fxfs_dax_catch_up's and fxfs_dax_map_moved's MAPs, which write_map_ns does not count. The VMM keeps no per-request timing, so the round trip's split between the guest's wakeup, the VMM thread and extentd remains unmeasured.

2026-11-03: parked, the user saying the sandbox team is happy with performance and integrating. Found on the way: the VMM's worker already times each request from the queue to its completion, by type (worker.rs Served), but logs it only as the worker stops, to firecracker.log, which the test results don't keep.

2026-11-06 tidy: priority high to low: TASK-146.1 was parked by the user on 2026-11-03 (the sandbox team happy with performance and integrating), and with writable DAX deleted (37c331a6c) a write(2) makes no window MAP; what a first store's PRIVATE round trip costs write(2) is TASK-92.24's.
<!-- SECTION:NOTES:END -->
