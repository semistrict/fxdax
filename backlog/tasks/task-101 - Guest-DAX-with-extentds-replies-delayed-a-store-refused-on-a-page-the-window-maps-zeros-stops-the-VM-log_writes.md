---
id: TASK-101
title: >-
  Guest DAX: with extentd's replies delayed, a store refused on a page the
  window maps zeros stops the VM (log_writes)
status: To Do
assignee: []
created_date: '2026-10-25 00:06'
updated_date: '2026-10-25 04:27'
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
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r89 (x10, same failpoints, kernel b9): round 4 of 4 failed in log_writes, Bad address 1 s after the 13th refusal at 0x318000. The page's history: MAP of staging extent ...916 from 0x0 writably (RESTAGE-shaped, 1 page) at 0x318000, then 0.15 ms later a MAP of the sealed ...913 from 0x8000 read-only at 0x318000: 913's region (window 0x310000, 9 pages, 8 mapped) reached into space 916's region had taken. r87 (pgbench-dax, 10 VMs on one daemon, no failpoints) stopped the same way: MAP ...735 writably 0x1c0000 bytes at 0x20800000, then MAP ...721 from 0xf8000 read-only at 0x20800000. Hypothesis: a read-only fxfs_dax_map_to, outside the gate, reads region->writable false and region->pages before fxfs_dax_seal shrinks it (seal changes both under regions_lock, map_to reads them under region->lock only), and maps the tail the seal gave back, which restage took. fxfs_dax_assert_mapped_within (WARN after a MAP past the region's pages) added; r90 checks it (x10 with failpoints, and 10-VM pgbench).

GCE r90 (x10, failpoints, assertion kernel): round 3 failed the same way, log_writes, 0x318000: MAP of ...862 writably (a whole 1-page extent in the range's own space, fxfs_dax_map_own), then 0.11 ms later MAP of the staging extent ...827 from 0x8000 read-only there; fxfs_dax_assert_mapped_within never fired, so not a region MAP past a seal's shrink. Hypothesis now: an evacuation's read-only MAP of a page (gathered under the file's sem, read, then sent with MAP_MANY outside it) lands after a write mapped the page anew in the range's own space; evacuee_place then sees the page moved and leaves it. Fix tried (217622edc, writes waiting for the file's evacuation MAP in flight) deadlocked in r91 round 1: the writer waits holding its file's sem in the gate; the evacuation gathering another file waits for that file's sem, held by a writer waiting for the gate, which a seal keeps closed for the first writer. The mutant without the wait passed 8 rounds of log_writes alone, so the race isn't shown either. ed59e7734 logs [DEBUG-ev] instead; r94 runs it. Deadlock-free fix if confirmed: per file, hold its sem read from gather through its MAP_MANY (writers then wait for the sem, the evacuation holding one file's alone, needing nothing more), giving up batching across files.

GCE r94 (tree rebased on main, [DEBUG-ev] logs, the evacuation fix not in it): fxfs-guest-checks-dax x10 with failpoints all green, no refusal round; pgbench-dax on 10 VMs one after another green. So no diagnostic hit either way. r95 runs 9c025a0ee (each file's runs mapped holding its semaphore) against the mutant evacuation_unlocked.
<!-- SECTION:NOTES:END -->
