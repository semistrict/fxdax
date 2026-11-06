---
id: TASK-164.10
title: >-
  Forked pgbench on copy-on-write DAX at 0.58 of no DAX: each write's PRIVATE a
  round trip, each of its pages a stage operation of its own
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 00:14'
updated_date: '2026-11-06 07:05'
labels:
  - performance
  - dax
dependencies: []
parent_task_id: TASK-164
priority: high
type: bug
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The perf gate's forked-dax (docs/measurements/dax-cow-gate.md) is 0.573 (r632) and 0.579 (r649): pgbench on a fork of a base's database, with DAX, runs at 802-870 tps against no DAX's 1,421; single-VM DAX is 0.995, as its files are created in the run and take the page cache. It blocks TASK-164.6. TASK-187's lift changes didn't move it; the worker restrictions were no cost (no failed KVM_RUN in r647, r649). Measured from r649's counters (run 2 of dax-forked, 60 s): 72,400 DAX write(2)s took 68.8 s, 31.9 s waiting for the inode lock and 33.9 s in iomap begin (468 us a write), the copy 2.4 s; 62,146 PRIVATEs of 746 MB (3 pages each); 30,085 MAPs (8.8 s, 294 us each, mostly rebinds); 75,161 pages rebound; the VMM's stage operations 65 us each (155k a minute, 10 s), its extentd client thread 22% of a core in the kernel; the vCPUs idle a quarter of the time. A read-only check query between runs took 21-29 s where no DAX takes 2.4 s.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Where the forked case's time goes measured, with counters where they were missing (the guest's PRIVATE time, the VMM's private stage operations)
- [x] #2 The cause named and fixed, with tests
- [ ] #3 The gate's single phase run on GCE: forked-dax before and after, 0.9 or better
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Counters: guest window_privates_ns and PRIVATE pages made ahead; daemon stats of the VMM's private stage operations (runs, pages, ns) told by STAGE_COUNTS.
2. Host: a PRIVATE's pages made in one change of the window, sent together; the VMM makes a run of consecutive private pages at once (one fill each, one mmap, one register, one wake, one prefault), and a page left out, its private memory already, filled without a map.
3. Guest: a write(2) that follows on from the file's last makes pages ahead of it private in the same PRIVATE (as readahead grows), so the WAL's appends take a round trip every so many pages, not every write.
4. Guest: a rebind maps all of a range's rebindable runs in one MAP_MANY, the file's semaphore taken a range at a time.
5. Tests for each; macOS workspace and Shuttle, Linux in Docker, GCE DAX suites jailed, then the perf gate's single phase (r650-669).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Where the forked case's time goes (AC1), from r649's counters, run 2 of fxfs-guest-pgbench-dax-forked (60 s, 802 tps), guest /proc/fs/fxfs deltas and the daemon's samples:
- Writes: 72,400 DAX write(2)s took 68.8 s: 31.9 s waiting for the inode lock, 33.9 s in iomap begin (468 us a write), the copy 2.4 s. Nearly every write asked for a PRIVATE: 62,146 of them, 746 MB (3 pages each). The log's segments (recycled, so read in from the base) and the tables' pages, written back, rebound and written again, all go through it.
- PRIVATE: the daemon spends 44 us on one; the VM process made each of its pages a stage operation of its own, 65 us each nested (155k operations a minute, 10.1 s), its extentd client thread 22% of a CPU in the kernel (136 s of system time over the case against 14 s without DAX); 74k of them overwrote pages installed (TLB flushes).
- MAPs: 30,085 a minute (8.8 s guest-side, 294 us each), 552 MB, mostly rebinds, 75,161 pages rebound a minute, a MAP per run of a page or two, under the file's semaphore held for writing through all of a file's ranges.
- Faults: 17-35k a minute, missing faults of pages a rebind or reclaim left uninstalled; fault-stores 22 in the whole case (writes take PRIVATE, not store faults); the worker restrictions cost no failed KVM_RUN (TASK-187).
- Window: 153 window_full and reclaims a minute with 1 GB of the 2 GiB window free, its largest free run 1.1 MiB (fragmentation): not fixed here.
- Guest: vCPUs idle a quarter of the time (guest idle jiffies 43% against 17% without DAX); the read-only consistency query between runs 21-29 s against 2.4 s.
Cause (AC2): a write's PRIVATE round trip, under the inode lock, on the log's flush path, costing the VM process a stage operation a page; and the log's appends each asking again. Rebinds and their MAPs a run each, the file's semaphore held through a file's, made it worse.
Fix, in the working tree (commits follow the GCE runs):
- Host: Window::make_private makes a PRIVATE's pages in one change; the VM process takes the consecutive STAGE_PRIVATEs together (wire::Receiver::take_privates, 64 at most) and SecondStage::make_private makes the run at once: one fill, one mmap, register, wake and pre-fault; a run with no page installed, withheld or restricted maps nothing (its memory is the private memory already).
- Guest: a write(2) following on from its file's last (begins where it ended or within its last 16 KiB) makes pages ahead of it private in the PRIVATE it needs, 16 doubling to 64, within the range, mapped and written pages only, below 7/8 of private_max.
- Guest: a rebind maps a range's every run in one MAP_MANY (fxfs_dax_rebind_range, from one fxfs_dax_maps_of look), the writeback lock and semaphore taken a range at a time, stopping at the target; the budget rebinds the files dirtied longest ago first.
- Counters: guest window_privates_ns and window_privates_hist, dax_private_ahead_pages; daemon stage-private-runs, stage-private-pages, stage-private-ns (STAGE_PRIVATE_COUNTS, 31, from the VM process every 500 ms and at close).
Tests: dax.rs ranges_are_made_private_at_once_within_the_bound (the run); wire private_pages_take_the_run_after_them, private_runs_bound_the_work_taken_from_the_socket; uffd Shuttle private_runs_map_at_once_once (red with a map always: 2 maps for 1), private_runs_race_loads_by_pct/_randomly, needs_private_runs_filled_before_mapped; extentd vm_processes_tell_the_daemon_what_their_windows_cost now with a private page; guest/sim cow_writes_ahead (red with nothing ahead: 128 PRIVATEs for 4) and cow_rebinds_map_ranges_at_once (red with a MAP a run: 32 requests for 1). cow_private_failed's pages reordered (5, 2): its retried write follows on from itself and made page 5 private ahead. requests_attach_nothing hung on STAGE_PRIVATE_COUNTS, which nothing replies to: excluded as ACK and STAGE_COUNTS are. Five sim mutants re-cut to the new code, same bugs (task-147-growing, task-79.28-before-147, task-164.4-rebind-shared, -writeback-unowned, task-164.9-private-failure-ignored); mutants.sh --check 34 of 34.
Blind spot: nothing measured what a PRIVATE cost the VM process, nor counted its pages per operation; the gate saw only tps. Structure: the stage's per-page interface (make_private of one page) made batching impossible below it; it takes a run now, as remap does for unmaps (TASK-91.16). Not fixed: window fragmentation (window_full with half the window free), worth a task if the gate still fails.
Local: macOS workspace 2,108 of 2,108 and every Shuttle part; Linux in Docker fxdax 471 of 475 (the container's 4: no git, the grants helper), fxdax Shuttle 63 of 63, clippy's disallowed_methods clean; guest/sim zloop every test and 3,000 seeds of each clean, mutants --check clean; upstream-changes.py --check OK.

GCE r650 (us-east4-a, jailed, release kernel), the perf gate's single phase at fedc14fd5 with this task's changes (now 08e2083ff, 5633ca4eb): every case passes; forked-dax 0.836 (dax-forked 1,175, 1,257, 1,177, 1,308, 1,217 against no-dax-forked median 1,455), from r649's 0.579 and r632's 0.573; dax 0.999, bigmap-read 5.13, the rest as docs/measurements/dax-cow-gate.md records. Rerun of the forked pair at 170662bad with the VM process's PRIVATE wait counted: 0.854 (1,216 against 1,424). Run 2 against r649's: write(2)s 21.9 s a minute (68.8), the inode's lock 6.6 s (31.9); PRIVATEs 26,068 (62,146), 486 us each guest-side: the VM process waits 320 us (daemon 83, its run of ~3.9 pages 135, the socket and scheduling ~100), the VMM's virtqueue path ~165 more; rebinds' MAP requests 655 (27,886). What is left is chiefly each data page's first store in the fork, a PRIVATE each by doc-13's design: TASK-164.11, with the options. AC3 not met: 0.84-0.85 against 0.9. The DAX suites (fxfs-guest-dax, -checks-dax, -checks-dax-small, -crash-dax, -crash-dax-killed, -crash-stray-dax) pass jailed on r650 at 5633ca4eb; macOS workspace 2,108 of 2,108 and Shuttle; Linux in Docker fxdax 471 of 475 (the container's 4) and Shuttle 65 of 65; guest/sim zloop clean, mutants --check 34 of 34.

Decision moved here from OPEN_QUESTIONS.md (2026-11-06): The perf gate's forked-dax was 0.58: each write(2) to a file read in from the base asks the host to make its pages private (`PRIVATE`), a round trip of ~470 us nested under the inode's lock, and the VM process made each page of it a stage operation of its own (65 us: a copy, an `mmap` over the page, a registration, a wake, a pre-fault). The log's writes, one after another, each asked again. Rebinding mapped each run of a page or two with a `MAP` of its own, the file's semaphore held through all of a file's. Decided:
- **A `PRIVATE`'s pages are one change of the window, and the VM process makes a run of consecutive private pages at once** (`wire::Receiver::take_privates`, `SecondStage::make_private` of a run), as it makes an unmap with its maps at once (TASK-91.16); the wire still carries a page an operation. A run with no page installed, withheld or restricted maps nothing: its memory is already the private memory, which filling makes the pages'. *Instead:* an operation of a run on the wire, with its pages' sources in a payload, which the receiver's batching refuses. *Undoing it:* `take_privates` and the window's run.
- **A write(2) that follows on from its file's last (begins within it or where it ended) makes pages ahead of it private** in the `PRIVATE` it needs anyway, 16 pages at first, doubling to 64, within the range, only pages mapped and written, and only below seven eighths of `private_max`, as readahead grows. Pages made so are private and clean, rebound as any. *Cost:* up to 64 pages copied that the log may never write, the last of a segment. *Instead:* doc-13's stage 5 left open whether to; a fixed run, or the whole range at a first write, which a table's random writes would take too. *Undoing it:* `FXFS_DAX_AHEAD_MOST` 0.
- **A rebind maps a range's every run in one `MAP_MANY`**, from one look at the file's extents, **and takes the writeback lock and the semaphore a range at a time**, stopping once the target is met; **the budget rebinds the files dirtied longest ago first** (`dirtied`, jiffies), where it went by inode number. *Instead:* a page-level least-recently-dirtied order (a clock a page), more memory and code for a table's uniform random writes, which no order helps. *Undoing it:* `fxfs_dax_rebind`'s loop back to one hold, and the sort.
<!-- SECTION:NOTES:END -->
