---
id: TASK-149.14.2
title: >-
  guest/sim's model of Fxfs: a writeback or clone that grows a file keeps what a
  cut left past its end
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 06:28'
updated_date: '2026-11-05 07:28'
labels:
  - guest-sim
dependencies: []
parent_task_id: TASK-149.14
priority: high
type: bug
ordinal: 634000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.14.1's zloop over seeds 138001-143000 on origin/main (f5a5a9d82): `failures 140272 1000: sim: PANIC in worker0: worker 0: ino 101 at 24046: read(2) has 0, and Fxfs's extents 0x41`. A truncate of a DAX file keeps its last block (FXFS_SET_KEEP_TAIL), whose bytes past the new end the window zeroes in its own page. Fxfs zeroes them itself as the file grows again (crates/fxfs txn_grow_size, zero_tail_before_growth: every growth but one that writes that block anew), and a write of part of a block reads what the block holds up to the end and zeros past it (read_unaligned_ends). The model of Fxfs did neither in fxfs_write_many or fxfs_clone_range: a writeback of the last page up to the end kept the old block's bytes past it, and a writeback past the end grew the file over them. Where the model differs from Fxfs a run proves nothing, and here it failed a run of a correct dax.c. Seed 140272 passes on origin/land-1604 by its schedule alone: the model there is the same.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every growth of the model (a write in the last block, a write past it, a truncate, a clone past it) reads zeros where a cut that kept its tail left bytes, as fxfs-posix's every_growth_finds_what_a_cut_left_past_the_end_zeroed has Fxfs do
- [x] #2 A scenario checks it, red with the model as it was, and a mutant puts each half back
- [x] #3 failures 140272 1000 passes on main with the fix; zloop over seeds 138001-143000 passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One growth in the model, as Fxfs's txn_grow_size: zeroes the old last block past the end unless the growth writes that block, used by write_inode, write_many and clone_range.
2. write_many's partial page reads its block up to the file's end, zeros past it (read_unaligned_ends).
3. Scenario fxfs_growths_zero_past_a_cut, the four growths of fxfs-posix's test, against the model; mutants for each half.
4. README, the simulator's own bugs and the mutants table; zloop and mutants runs.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## How it was found

TASK-149.14.1's zloop over the hunt's shard, seeds 138001-143000, on origin/main f5a5a9d82: `failures 140272 1000`, after check_whole's fsync, `ino 101 at 24046: read(2) has 0, and Fxfs's extents 0x41`. It replays on x86-64 and arm64 alike. Traced in an instrumented copy (not committed) with -W 101 5: the big file was cut from 2845892 to 24046 bytes with FXFS_SET_KEEP_TAIL (data.c, a DAX file's truncate), so Fxfs's block for page 5 kept the old bytes past 24046. Three writebacks of page 5 followed, each a part 20480+3566 (dax.c writes the last page up to i_size), and the model filled the rest of each new block from the old one, past the end too. The last ran in the syncer while worker 0's write at 31756 past the end was in progress, before the write grew i_size. The fsync after wrote pages 7-10, and the model grew the file to 41264 (`file->size = max(file->size, end)`) over 24046-24575, which read(2) read as zeros, rightly.

## Root cause: the model, not dax.c

Fxfs (crates/fxfs) zeroes what a cut left in the last block every way the file grows, unless the growth writes that block anew: txn_grow_size for writes and write_many, zero_tail_before_growth for clones and allocations. A write of part of a block reads the rest with read_unaligned_ends, which zeroes past the file's size. fxfs-posix's every_growth_finds_what_a_cut_left_past_the_end_zeroed tests exactly this for Fxfs. The model zeroed only a write_inode growth. dax.c relies on Fxfs here, rightly: with Fxfs's rules the run is correct.

## Blind spot

The model is a second writing of Fxfs, and nothing checks it against Fxfs. Its growth rule had no test. A failing run needs a cut that keeps a tail, a writeback of the last page up to the end, and a growth that comes through writeback before any write_inode or truncate growth: here a syncer's writeback between a write's zeroing and its growth of i_size. One run in 5,000 met that.

## Fix and tests

- 0a6562c7a: refactor, one growth function (grow, set_size), no change to a run: failures 140272 and interleave 139296 hash as before (53933b316c8421fd, 649ee7e0395436e3), and every scenario passes.
- 0cb6928f7: zero_before_growth and grow(…, writes_last), used by write_inode, write_many and clone_range (the clone zeroes before sharing blocks, as zero_tail_before_growth, which a clone of the last block itself needs). read_kept for write_many's partial pages. Scenario fxfs_growths_zero_past_a_cut: fxfs-posix's growths (a write in the block, a write past it, a truncate, a clone past it) and a clone of the last block past it, against the model, every byte of the first page compared.
- Red: the scenario before the fix, `a write in the block: byte 1000 is 0x63, not 0`. Each mutant puts one half back and is caught by it: task-149.14.2-growth-keeps-tail (`a write past it: byte 1000 is 0x63`), task-149.14.2-partial-write-keeps-past-end (`a write in the block: byte 1000 is 0x63`).
- Green on land-1604 + fix: zloop -s 3000 (CI's seeds, every test and scenario) 0 failures; zloop -f 138001 -s 5000, 0 failures; mutants.sh --check 31, 0 broken; mutants.sh, 2,000 seeds: every mutant caught but the five README calls equivalent (task-153.1-fault-limit, patch-0012-invalidate-unmaps-after, task-79.28-range-too-small, task-164.4-truncate-unwritten, task-44.19-map-to-unlocked); task-160.2-resize-no-rollback now at seed 8, not 1 (README updated).
- On main with the same model change applied in a copy: failures 140272 1000 passes.

## Look around

- The model's other growths: write_inode (zeroed already), write_many and clone_range (fixed). It has no fallocate. fxfs_punch frees whole blocks only; data.c zeroes partial pages in the window, as Fxfs's punch is asked whole blocks, so no difference found.
- A clone of a source's last block whose tail a cut kept: the kernel's remap checks let an unaligned source end only at the destination's end, so the bytes stay past it, and the destination's next growth zeroes them now.
- Coverage (guest-coverage.py) is of fs/fxfs on a kernel and says nothing of the model; not run.
- The class, the model drifting from Fxfs, has happened before (interleave -C seed 32826, a seal's extents called open past its COMMIT). Filed TASK-149.29: the model against fxfs-posix, same operations, sizes and bytes compared.

## Structure

The model had three places that grew a file, one of which zeroed. It now has one, as Fxfs has txn_grow_size (TASK-153.4 gave Fxfs its one owner of a size). The deeper problem is that the model exists at all: README's plan, linking the real fxfs-linux, would make the class impossible. Until then TASK-149.29 would catch drift as it is made.

## Base

The seed fails only on main, but the model bug is the same on land-1604, which lands next. Per the brief's rule for a bug on both, the fix is on land-1604 (hunt-139296 = origin/land-1604 + 2 commits). It applies to main with two hunks by hand: main's fxfs.c has fxfs_setattr, not write_inode, and no attr_of.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The model of Fxfs grew a file in a writeback (fxfs_write_many) or clone over what a cut that kept its last block (a DAX truncate) left past the end, and a writeback of part of a block kept that too, where Fxfs zeroes both (txn_grow_size, zero_tail_before_growth, read_unaligned_ends). That failed a correct dax.c in failures 140272 at 1,000 on main. A refactor (0a6562c7a, runs unchanged) made one growth function; the fix (0cb6928f7) routes every growth through it and zeroes a partial block past the end. Scenario fxfs_growths_zero_past_a_cut was red before the fix and is green after, and its two mutants are caught. On land-1604 + fix: zloop of seeds 1-3,000 and 138001-143000 pass, and mutants.sh catches all but the five known equivalents. On main with the fix, 140272 passes. DoD #1 (workspace on macOS and GCE) not run: the change is guest/sim only, which the workspace does not build.
<!-- SECTION:FINAL_SUMMARY:END -->
