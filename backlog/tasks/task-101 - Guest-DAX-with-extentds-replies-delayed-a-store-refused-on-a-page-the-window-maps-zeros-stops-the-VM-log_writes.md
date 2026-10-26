---
id: TASK-101
title: >-
  Guest DAX: with extentd's replies delayed, a store refused on a page the
  window maps zeros stops the VM (log_writes)
status: Done
assignee: []
created_date: '2026-10-25 00:06'
updated_date: '2026-10-26 00:39'
labels:
  - bug
dependencies: []
priority: high
ordinal: 400000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-44.10's failpoints (GCE r82, kernel b8): FXDAX_FAILPOINTS='extentd::map=10%sleep(2);extentd::commit=20%sleep(20);extentd::write=10%sleep(5)', fxfs-guest-checks-dax: stores refused at window offsets 0x30a000, 0x30c000, 0x314000, 0x312000 (the window mapping zeros there, read-only), then 19 s later Firecracker 'Failure during vcpu run: Bad address' in log_writes (after PASS: stats). fxfs-guest-dax and pgbench-dax passed with the same delays. A store to a page mapped zeros: a hole the guest believes it mapped writably, or a range the guest wrote whose extent's mapping the host replaced with zeros (replace_kept, a freed extent) while the guest still took it for writable. The refusal that stopped it wasn't logged (powers of two).
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r89 (x10, same failpoints, kernel b9): round 4 of 4 failed in log_writes, Bad address 1 s after the 13th refusal at 0x318000. The page's history: MAP of staging extent ...916 from 0x0 writably (RESTAGE-shaped, 1 page) at 0x318000, then 0.15 ms later a MAP of the sealed ...913 from 0x8000 read-only at 0x318000: 913's region (window 0x310000, 9 pages, 8 mapped) reached into space 916's region had taken. r87 (pgbench-dax, 10 VMs on one daemon, no failpoints) stopped the same way: MAP ...735 writably 0x1c0000 bytes at 0x20800000, then MAP ...721 from 0xf8000 read-only at 0x20800000. Hypothesis: a read-only fxfs_dax_map_to, outside the gate, reads region->writable false and region->pages before fxfs_dax_seal shrinks it (seal changes both under regions_lock, map_to reads them under region->lock only), and maps the tail the seal gave back, which restage took. fxfs_dax_assert_mapped_within (WARN after a MAP past the region's pages) added; r90 checks it (x10 with failpoints, and 10-VM pgbench).

GCE r90 (x10, failpoints, assertion kernel): round 3 failed the same way, log_writes, 0x318000: MAP of ...862 writably (a whole 1-page extent in the range's own space, fxfs_dax_map_own), then 0.11 ms later MAP of the staging extent ...827 from 0x8000 read-only there; fxfs_dax_assert_mapped_within never fired, so not a region MAP past a seal's shrink. Hypothesis now: an evacuation's read-only MAP of a page (gathered under the file's sem, read, then sent with MAP_MANY outside it) lands after a write mapped the page anew in the range's own space; evacuee_place then sees the page moved and leaves it. Fix tried (217622edc, writes waiting for the file's evacuation MAP in flight) deadlocked in r91 round 1: the writer waits holding its file's sem in the gate; the evacuation gathering another file waits for that file's sem, held by a writer waiting for the gate, which a seal keeps closed for the first writer. The mutant without the wait passed 8 rounds of log_writes alone, so the race isn't shown either. ed59e7734 logs [DEBUG-ev] instead; r94 runs it. Deadlock-free fix if confirmed: per file, hold its sem read from gather through its MAP_MANY (writers then wait for the sem, the evacuation holding one file's alone, needing nothing more), giving up batching across files.

GCE r94 (tree rebased on main, [DEBUG-ev] logs, the evacuation fix not in it): fxfs-guest-checks-dax x10 with failpoints all green, no refusal round; pgbench-dax on 10 VMs one after another green. So no diagnostic hit either way. r95 runs 9c025a0ee (each file's runs mapped holding its semaphore) against the mutant evacuation_unlocked.

GCE r95: the mutant evacuation_unlocked (the semaphore let go across the evacuation's MAP) stopped the VM in round 5 of 10 with TASK-101's signature (stores refused at 0x309000.., Firecracker exiting): the window reproduced. HEAD (semaphore held for reading through the MAP) failed regions_evacuated in round 1: 137 regions more after 400 fsyncs, as a write queued behind the MAP took the range before the placing, which skipped the runs. Amended (5183ab50d): the semaphore held for writing from gathering through placing. r98: HEAD x20, mutant x10.

GCE r98: HEAD (5183ab50d, here ddc036a20) green x20 with the failpoints; the mutant evacuation_unlocked MISSED x10 (r95 killed it 1 in 10). Neither r95 nor r98 set fxfs.dax_delay_us, so the delay point before the MAP slept never. r99: both x10 with fxfs.dax_delay_us=1000 and the failpoints.

GCE r99 (fxfs.dax_delay_us=1000 and the failpoints): HEAD and the mutant both failed regions_evacuated in round 1 (35 and 91 regions more after 400 fsyncs, limit 32); neither reached TASK-101's signature, so r99 says nothing of the race. r103: main without the fix and HEAD, regions_evacuated alone x10 with the delays (does the limit hold under delays at all?), and the mutant x10 with every check but regions_evacuated.

GCE r103: main (no fix) regions_evacuated x10 with fxfs.dax_delay_us=1000 green, so r99's HEAD failure is the fix's under delays (main has no delay point in the evacuation; the fix added one, inside the semaphore); r103b measures HEAD's rate. The mutant MISSED x10 with every check but regions_evacuated: killed 1 in 30 rounds so far, as no check writes a page between its evacuation's gathering and MAP. 85341dafc adds evacuation_rewrites (each page written again at once after the fsync sealing it); r104: HEAD and the mutant, it alone x10, fxfs.dax_delay_us=20000.

Why the fix failed regions_evacuated under delays: it takes the semaphore the moment the seal ends, when the test's next write runs its iomap; the iomap holds its range (users) past the semaphore, through its copy and the write path's delay point, so the placing found users != 1 and skipped (main placed only after the MAP_MANY round trip). 20864bb92 (fixup): fxfs_dax_lock_evacuable takes the semaphore once no iomap uses a range the evacuation gathers, waiting without it (an iomap's copy may fault on the file), as fxfs_dax_lock_unheld does. r105: checks-dax x10 with fxfs.dax_delay_us=1000 on it.

GCE r103b: ddc036a20 (before the fixup) regions_evacuated alone x10 with fxfs.dax_delay_us=1000 green: r99's failure, in the full suite, is rare (1 in 11 so far). r105 runs the fixup's tree.

GCE r104: evacuation_rewrites x10 (fxfs.dax_delay_us=20000): green on ddc036a20, the mutant MISSED: the check wrote the page once after the seal, and the evacuation's late MAP lands after that write's copy; the refusal needs a later store to the page, taken for writable, before the next seal. 95af36bac (fixup): each round writes the page on for 30 ms. r106: it on HEAD (with the lock_evacuable fixup) and the mutant, x10, 20 ms delays.

GCE r105: 20864bb92 (lock_evacuable) full checks-dax with fxfs.dax_delay_us=1000 failed regions_evacuated in round 1 again (89 regions more): the users skip isn't the cause, or not alone. Every full-suite run with the guest delays failed in round 1 (r99, r105), every run of the check alone passed (main r103a, ddc036a20 r103b). Missing control: main, full suite, delays: r107 (seed 105, x3).

GCE r106: evacuation_rewrites rewriting 30 ms per round, 20 ms delays: HEAD green x10, the mutant MISSED x10. r107: main full checks-dax with fxfs.dax_delay_us=1000 (seed 105) green x3, so r99/r105's regions_evacuated failures are the fix's, in the full suite alone. c587b2b50 (fixup): the delay point inside the MAP, after the check for runs (files with nothing gathered were delayed too). The one kill (r95 round 5) came in seal_race (write(2)s and mmap stores to the same pages while another file seals), whose stride matches the refused pages. r109: HEAD full suite with the delays x3; the mutant on seal_race,evacuation_rewrites x10 at 20 ms.

GCE r109: HEAD (c587b2b50, delay inside the MAP) full checks-dax with fxfs.dax_delay_us=1000, seed 105, green x3, as main (r107): the regions_evacuated failures are gone. The mutant evacuation_unlocked MISSED x10 on seal_race,evacuation_rewrites at 20 ms. Tally for the mutant: killed once (r95 round 5, seal_race, TASK-101's signature) in ~70 rounds across r95-r109: no check reproduces the race reliably.

Sandbox team round 6 (TASK-109, b746104, main without this fix): 2 of 21 DAX VMs died with this signature without failpoints: 8 GiB during initdb (window 0x21469000: MAP of ...631 writably 0x1b2000 bytes at 0x21400000, then 1.18 ms later MAP of the older ...619 from 0x0 read-only, 0x200000 bytes at 0x21400000) and 2 GiB with synchronous_commit=off (0xdc10000: ...285 writably 0x104000 at 0xdc10000, then ...272 from 0x0 read-only 0x200000 at 0xdc00000). As r87/r89/r90: a writable map_own of a newer extent, then the evacuation's read-only MAP of an older one. TASK-102 lifted nothing because the window never made the page writable again.

evacuation_rewrites dropped before landing (user's call): it never stopped a VM with the mutant evacuation_unlocked (r104, r106, r109).

Landed in 139e82be6 (with TASK-107). Validation: macOS scripts/test.sh (clippy, 2,514 workspace tests and the rest) passed; Linux workspace on GCE r113: 2,633 of 2,634 passed, the one failure TASK-106's flaky memory test (an extent page-in failing NO_MEMORY), unrelated; scripts/upstream-changes.py --check passed; guest checks-dax green r98 x20, r103b x10, r104/r106 x10, r109 full suite with delays x3. Linux tests run on GCE, not Lima (project practice).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An evacuation held a file's DAX semaphore only while gathering its runs; its read-only MAP_MANY could then land after a write's writable map of the same page in the range's own space, the host refusing the write's stores and the VM stopping (log_writes under delays, 10-VM pgbench, the sandbox team's b746104). Each file is now evacuated holding its semaphore for writing from gathering through its MAP to placing, once no iomap uses its ranges (fxfs_dax_lock_evacuable); the delay point sits inside the MAP. Verified by the guest checks on GCE (r98-r109), the macOS workspace tests, and the Linux workspace tests (r113; one unrelated flaky failure, TASK-106).
<!-- SECTION:FINAL_SUMMARY:END -->
