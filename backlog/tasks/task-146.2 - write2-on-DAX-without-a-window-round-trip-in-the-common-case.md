---
id: TASK-146.2
title: write(2) on DAX without a window round trip in the common case
status: Done
assignee: []
created_date: '2026-10-31 22:15'
updated_date: '2026-11-03 02:10'
labels: []
dependencies: []
parent_task_id: TASK-146
priority: high
ordinal: 498000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Their ask 2. 122,027 of 134,078 maps per run are evacuation's, not the write's own: find what write(2) itself waits on first (its 605 us includes waits behind evacuation). Serving write(2) through the device WRITE path isn't their 72 us: that is the page cache, which a DAX inode hasn't, and a device WRITE is a 340+ us round trip. See TASK-131.2, TASK-109.1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 write_iter's time split: own MAPs, waits behind evacuations, copying
- [ ] #2 A chosen design moves pgbench-dax past no DAX's synchronous_commit=off tps, or says why it can't
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31, GCE r364, pgbench-dax with every commit asynchronous, tps 2545/2536/2560/2623/2569 (their 2436): 1,409,801 writes, 251 us each: waiting for the inode's lock 113.7 us (45%), mapping under it 44.6 (18%), Fxfs 31.2 (12%), the gate 10.5 (4%), the rest, the copy among it, 51 us. 0.98 Fxfs transactions a write. Evacuation: 994,451 of 1,203,185 maps (83%). A MAP request 674 us; a device read 1089 us, a WRITE 1537 us, against about 500 and 340 uncontended: the VMM's one thread is behind evacuation. So the writes queue behind each other: a MAP about 1 write in 15 holds the exclusive inode lock for 674 us. Next: rarer write-path MAPs (region step), then the inode lock shared for overwrites (ext4 DAX does), then evacuation's traffic.

2026-11-01: correction: ext4 and XFS take the inode lock exclusive for DAX writes, as far as I recall: only their direct I/O shares it for overwrites. An earlier note and message said ext4's DAX shares it: wrong. Absent-page evacuation measured and dropped: dead-ends/evacuation-leaving-pages-absent.md.

2026-11-01, sandbox team at fc281d8: 67,170 writes, 35.0 s, about 520 us each; map 6.7, lock 5.6, Fxfs 4.3, gate 2.5, 15.9 s (45%) unattributed; they ask that the breakdown cover the rest, the copy included. On test/flaky-writers: dax_write_setup_ns, _fault_in_ns, _begin_ns, _begin_other_ns, _copy_ns (dax_iomap_rw less its iomaps', a store's EPT faults among it), _end_ns, _retry_ns, _tail_ns, _other_ns, and write_wait_ns, already counted, now only for write(2). Guest check write_times_add_up asserts other under 10% of the total. Compiles for guest and host; not yet run on a kernel. The read-fault hang in the same report is thread 'DAX Read Faults Hang Under Pressure', a separate session.

r428 (before the audit's fixes), fxfs-guest-pgbench-dax, 1,041,386 writes, 364.29 s, 350 us each, tps 1027 to 1184: begin 247.9 s (68.0%), of which begin_other 102.3 (28.1%), map 75.9 (20.8%), fxfs 46.2 (12.7%), gate 23.5 (6.4%), wait 0.01; lock 97.9 (26.9%); copy 14.8 (4.1%); setup 1.7, end 0.4, fault_in 0.2, tail 0.06, retry 0.02; other 1.3 (0.36%). It adds up. The copy, which the sandbox team asked about and I guessed was the gap, is small. Beside it: 216,564 seals, window_maps 1,232,207 costing 248.6 s, 755,798 of them evacuation's, 257,297 evacuations. Next: begin_other is fxfs_dax_prepare_write and fxfs_dax_get_range, untimed there: r450 times them, on the merged tree. write_times_add_up failed in r428 on a wrong stat name of mine, dax_write_ns for dax_writes_ns, not on the sum: fixed, unproven until r450.

r450, the merged tree, pgbench-dax: 1,031,541 writes, 365.7 s, 354.5 us each, tps 1000 to 1176: fxfs_dax_get_range 101.2 s (27.7%), inode lock 99.6 (27.2%), map 71.5 (19.6%), fxfs 46.0 (12.6%), gate 23.5 (6.4%), copy 14.4 (3.9%), begin_other 3.9, other 1.4 (0.4%), prepare 0.5. write_times_add_up passes. Audit fixes against r428: 354.5 against 349.8 us, tps alike; different zones, so rough. Hypothesis, r453 testing: get_range's time is its first down_read of file->sem, behind evacuations, which hold it for write: dax_evacuate_hold_ns 180.6 s over 255,755 evacuations, 706 us each, 110.8 s of it MAPs. Beside it 214,929 seals, 748,928 evacuation MAPs.

r453: writes wait 83.0 s of 403 s behind evacuations for file->sem, the first down_read in fxfs_dax_get_range: the wait AC1 asked for, which write_wait_ns never counted: dead-ends/theory-wal-writers-wait-on-evacuation.md's counters missed it, and its 'writes hardly wait' no longer holds. r458 against r459 (main, one zone, 3 rounds a side): fxfs.dax_evacuate_every=16 gives 1,061 tps against 987, round means not overlapping, writes 362 against 384 us, wait behind evacuations 57 against 78 s, holds 119 against 178 s, evacuations 100k against 233k, MAPs not fewer, writes' own MAP time 26% up. Against dead-ends/batched-evacuation.md, which measured no gain on 2026-10-31. Two hosts, so it may be the host: r491 (batched, with delay injection and full scripts) and r492 (control) repeat it on other hosts.

Batching settled: four hosts, one zone, three rounds each: batched 1061, 1054 tps; control 987, 1021: about +5%. Recorded in dead-ends/batched-evacuation.md. The cost is per page. AC1, the split, is met by the counters; AC2 stays open. Cheaper per page is TASK-153.6; MAP with no semaphore is TASK-153.10.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded. A write(2)'s time is split by counters that add up (c5ab5a789, 2e10c432b, b940ca91e, 9e83708b3; write_times_add_up, r450, AC1). r453 showed writes waiting behind evacuations; batching evacuation gave ~5% (dead-ends/batched-evacuation.md). The design behind AC2 was replaced by 106b6128d (a write takes no MAP of its own) and fafefbac7 (a page stays at one window address, evacuation removed; 1,071 against 1,011 tps). Not shown: pgbench-dax beating no-DAX with synchronous_commit=off.
<!-- SECTION:FINAL_SUMMARY:END -->
