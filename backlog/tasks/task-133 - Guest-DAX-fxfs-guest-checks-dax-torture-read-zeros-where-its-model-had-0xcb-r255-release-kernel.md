---
id: TASK-133
title: >-
  Guest DAX: fxfs-guest-checks-dax torture read zeros where its model had 0xcb
  (r255, release kernel)
status: To Do
assignee: []
created_date: '2026-10-30 01:36'
updated_date: '2026-10-30 09:44'
labels:
  - dax
  - bug
dependencies: []
priority: high
ordinal: 480000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r255 (n2 nested, release guest kernel), fxfs-guest-checks-dax round 1 at faeec5186 (main 83c92d55f + vendor/firecracker 43e203825, which pre-faults MAP_AHEAD's read-only maps after the MAP's reply, TASK-131.3): 'seed 1794450854429239374, phase 6, after the threads: /mnt/fxdax/torture-1 at 0x1e06b (thread 1's) reads 0x0, its model 0xcb'. 101 refused stores in firecracker.log, none unexplained. Not reproduced since: with that change, torture alone 10 of 10 rounds and the full suite 6 of 6 more; without it (vendor/firecracker 38cb48619), the full suite 8 of 8. Whether the change makes it likelier, or it was there before, is open (TASK-131.3 is measuring more rounds). Logs: /tmp/fxdax-r255-fork-suites/e2e/fxfs-guest-checks-dax/round-1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause is found, or the failure shown independent of the deferred pre-fault by rounds with and without it
- [ ] #2 FXDAX_SEED=1794450854429239374 FXDAX_CHECKS=torture and the full suite pass 20 rounds on the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Second failure with the deferred pre-fault (vendor/firecracker 43e203825), r255 at faeec5186, full suite round 1 of 12: append_while_mapped 'byte 0x74000 of 0x90000 mapped' read 0, its append wrote 2. So with the change: 2 zero reads in 9 full rounds (plus 10 of 10 torture-only); without it, 8 of 8 full rounds clean. The change is implicated: a KVM_PRE_FAULT_MEMORY of a MAP_AHEAD range run after the MAP's reply races the guest's next window changes there (UNMAP, a writable MAP of the same space); the page tables it reads may be mid-change, or a page not present faults through the window's userfaultfd from the replies thread. The same pre-fault before the reply can race other requests' changes too, only in a narrower window: whether the race exists before the change at a lower rate is the open question. 43e203825 is not pushed.

2026-10-30, r255 at 6a5514ed0: main's Firecracker 38cb48619, 20 rounds passed; the late pre-fault 43e203825, append_while_mapped in round 3 of 20; early pre-fault with the receive thread held as long as a late one holds it (dbbbfd82c, timing only), round 2 of 20 a guest soft lockup: guest-agent 26 s on the device's lock, in fxfs_device_seals from fxfs_dax_iomap_write, every section of that lock a few instructions, so a holder stopped or a lock corrupted; the console ended before CPU 1's state, so neither is known. Neither zero read in that run. Seen once (/tmp/fxdax-r255-t133-delay/e2e/fxfs-guest-checks-dax/round-2).

Cause of append_while_mapped: a write past the file's end grew i_size as it began, before its bytes: a reader that found the file longer mapped and read the zeros of blocks the copy hadn't reached (byte 0x74000 of 0x90000: the third 192 KB append, 0x60000..0x90000). A late pre-fault only widened the window. Fix: i_size grows in fxfs_dax_iomap_end by what was written, in the gate, as ext4's and XFS's DAX writes; fxfs_dax_map_chunk maps as far as the write grows the file. Fxfs still takes the size in fxfs_dax_map_write as the write begins, so a write that stops short brings it back (fxfs_dax_set_size), which the first draft of the fix left past the inode's. Tests: append_size_follows_data, a write paused with its iomap made; append_stopped_short, an append whose user memory ends part way. This does not explain the first failure, torture's 0x0 for 0xcb 'after the threads', when no write is in flight: still open.

Guest tests now dump every CPU on a soft lockup (c49d18b87, local, softlockup_all_cpu_backtrace, checked by lockups_show_every_cpu), to name the holder of what a stuck CPU waits for; r271 repeats the timing-only variant on it.

Torture's zero read isn't the size's order: its files keep their size through the threads (set_len only between phases), and the byte, 0x1e06b of torture-1, is in a slot only thread 1 writes, in a shared page, so a whole write or store there was lost, or the page read is an older one. Seen only with the late pre-fault (one in ~30 rounds with it, none in 28 without). So 43e203825 stays out until it is cleared on its own, whatever r266 shows: one clean set of 20 rounds doesn't rule out a failure this rare.

macOS at 76ec99cf9, scripts/test.sh (clippy, workspace, scripts' tests): 1834 + the rest passed, none failed; scripts/upstream-changes.py --check passed.

Mutants at 76ec99cf9: r267 without the short write's fix, append_stopped_short: 12288 bytes after the inode was read in again, 8192 written; r268 with the size grown as the write's iomap is made, append_size_follows_data: 69632 bytes at the pause, 4096 expected. Each test fails for the bug it names.

r266, the fix (76ec99cf9) with the late pre-fault 43e203825, full suite: torture failed in round 6 of 20: seed 1793086257549853554, phase 20, after the threads, torture-0 at 0xb016 (thread 2's) reads 0x0, its model 0x10. So torture's zero read is a bug of its own, with no append in it, as the note above says: the size's order wasn't it. 2 torture failures now, both with the late pre-fault. r274 repeats torture with fxfs.dax_trace on, whose dump the check takes before it fails.

r265, the fix (76ec99cf9) on main's Firecracker, full suite: failed in round 13 of 20: the host's fsck: ExtentExceedsLength(9, 2162, AttributeId(0), 3242615, Value("3223552..3276800")), an extent to a 64 KiB boundary past the file's end: blocks fxfs_dax_append_end reserved (TASK-92.58), most likely of random-ops, which alone is left for the host. In no other saved run. Open whether 76ec99cf9 causes it: r275 at it and r276 at 3358b1089, 40 rounds each of random_ops. r270, Linux workspace at 76ec99cf9: 1964 of 1965; frozen_vm_processes_hold_nothing_up timed out at 30 s ('the frozen VM process lives on', extentd/tests.rs:1199), in code the commit doesn't touch; not yet shown a flake.

Cause of r265's fsck error, to prove on GCE: blocks fxfs_dax_append_end reserves past a file's end (898253757, TASK-92.58) are extents past its length at rest, which fsck calls ExtentExceedsLength, on any file left with them; only random-ops is left at the host's fsck. fsck/store_scanner.rs (fxdax) now allows them to the next 64 KiB boundary (max_extent_end, the guest's FXFS_DAX_APPEND_SIZE), as a crash leaves them and no trim could; test_blocks_reserved_past_the_end fails without it with that error. Guest check append_reserved_tail leaves such a file, and the host's check now runs fsck on runs filtered by FXDAX_CHECKS, which skipped it (so the mutants' and any filtered run's images were never checked). r275 main, r276 3358b1089, both strict; r277 the fix, r278 the fix, 20 full rounds.

r269: seed 1794450854429239374, torture, 20 rounds passed (main's Firecracker). r271: the timing-only variant dbbbfd82c with every CPU dumped on a lockup, 20 rounds passed: the lockup didn't recur. r274: late pre-fault, torture alone, fxfs.dax_trace=1, 40 rounds passed: not reproduced, so its failures came in full suites only (2 in ~35 full rounds). r275's check made no tail (reservations stop at the range's end, and a small file's range does), so proved nothing; append_reserved_tail now shrinks a 1 MiB file first and asserts the blocks. r279 main and r280 3358b1089, strict; r281 with fsck's allowance.

Proved: append_reserved_tail, strict fsck, fails with ExtentExceedsLength(9, 4, AttributeId(0), 274532, Value("274432..327680")) at main (r279) and at 3358b1089 before the size fix (r280), each round: the error r265 hit is the reservation's since 898253757 (TASK-92.58), any file left with a reserved tail, not caused by 76ec99cf9. Its assertion of the blocks passed: the tail is there.
<!-- SECTION:NOTES:END -->
