---
id: TASK-149.29
title: >-
  guest/sim's model of Fxfs run against fxfs-posix: the same operations on both,
  each file's size and bytes compared
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 06:37'
updated_date: '2026-11-06 06:56'
labels:
  - guest-sim
dependencies: []
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 635000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
guest/sim tests dax.c against a second writing of Fxfs (guest/sim/fxfs/fxfs.c), and where the model differs from Fxfs a run proves nothing or fails a correct dax.c. Nothing checks the model against Fxfs: its drifts show only as sim failures that take hours to reduce. Two so far: a seal's minted extents called open past the COMMIT (interleave -C seed 32826, README's simulator bugs), and growths that kept what a cut left past the end, where Fxfs zeroes it (TASK-149.14.2, failures 140272), which fxfs-posix's every_growth_finds_what_a_cut_left_past_the_end_zeroed already said of Fxfs. A differential run (the model's fxfs.h calls, write_many, write_inode with and without FXFS_SET_KEEP_TAIL, clone_range, punch, fsync, against fxfs-posix's Fs of the same operations) would find a drift as it is made, before a seed meets it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Random sequences of the operations dax.c and data.c call of Fxfs run on the model and on fxfs-posix, and each file's size and bytes up to it agree after every operation
- [x] #2 With task-149.14.2-growth-keeps-tail or task-149.14.2-partial-write-keeps-past-end in, it fails
- [x] #3 It runs in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New workspace crate crates/fxfs-sim-diff: build.rs compiles guest/sim/fxfs/fxfs.c (or FXDAX_SIM_MODEL, a mutant's copy) with cc against a small shim of its own (single-threaded locks that panic if contended, an extent store as kernel/device.c's: lengths, seals, frees, no write to a sealed extent), so it builds on macOS and Linux and runs in the workspace's nextest, CI's workspace part.
2. A proptest of sequences of what dax.c and data.c call (write_many with and without dirty room, partial blocks; write_inode cuts with and without KEEP_TAIL, growths; clone_range, to the source's end unaligned too; punch; fsync; failed commits; remounts of Fxfs) on the model and on fxfs-posix's Fxfs over a FakeDevice. After each: size, bytes up to it, every block whole through the extents (past the end too), holes, blocks shared, and Fxfs open => model open.
3. Ops interleaved at the model's own scheduling points (a seal's COMMIT and end, write_many's copy, write, reply and commit), as its locks allow, applied to Fxfs before or after as the point says: for the seal drift (seed 32826).
4. Red: guest/sim/mutants/task-149.14.2-*, and a mutant of the seal bug put back; a mutants.sh for the crate, --check in scripts part.
5. Run many seeds; fix whichever side is wrong, with a test.
6. OPEN_QUESTIONS decision; README; notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: crates/fxfs-sim-diff, a workspace crate. build.rs compiles guest/sim/fxfs/fxfs.c unchanged (or FXDAX_SIM_MODEL, a mutant's copy) with cc over shim/ (libc memory; locks that never wait and fail the run if taken held; kernel/device.c's extent store: lengths, seals, frees, refusing a write to a sealed extent). A sim_panic unwinds (C-unwind, -fexceptions) to proptest, which shrinks. diff_tests: proptest sequences of fxfs.h's calls on the model and on fxfs-posix's Fxfs over an awkward FakeDevice: write_many (whole and partial pages, to the size, with and without dirty room, failing between bytes and commit via fxfs_set_fail_commits/FAIL_COMMITS), write_inode cuts with/without SET_KEEP_TAIL and growths, clone_range (blocks and to the source's end), punch, fsync, remount (Fxfs only). After each op: sizes, read(2) bytes, every block whole via extents (past EOF too), holes, shared blocks, Fxfs-open => model-open. Ops interleave at the model's sim_points (a seal's COMMIT/end, writeback's copy/write/reply/commit) as its locks allow, applied to Fxfs before or after per point. Choice recorded in OPEN_QUESTIONS.md.

Found:
1. Model drift: move_tail moved the last block on every growth past an unaligned end (and cut without KEEP_TAIL) even when it held zeros past the end; Fxfs's txn_zero_tail leaves such a block. Bytes alike, but the model's block went to a new open unshared extent, Fxfs's stays sealed and shared: the sim never exercised a growth keeping its last block mapped. Fixed in fxfs.c; mutant task-149.29-tail-moved-though-zero.
2. REAL FXFS BUG: write_dirty (write_many_reserved) growing a file past an end within a block a KEEP_TAIL cut left non-zero rewrites that block (txn_grow_size -> txn_zero_tail), but reserved_transaction_options counted only the written blocks: NoSpace on an empty filesystem. On a guest: a DAX file truncated (keeps tail), then written past the next block, whose writeback lands in a write_many without the old last page, fails ENOSPC. Blind spot: txn_zero_tail's doc called it accepted ('unless a cut left one so'); no test paid a growth with dirty room after a kept tail. Fix: one function decides whether a growth zeroes the tail (tail_zeroed_by_growth), used by txn_grow_size, zero_tail_before_growth and write_many_paid's room. Structure: the class (room computed apart from what the transaction allocates) is now one owner for this case; other hidden allocations in reserved transactions would need the same; none found in write_many_paid's path. Test: fxfs-posix tests::a_writeback_paid_with_dirty_room_has_room_for_the_tail_its_growth_zeroes, red before the fix (allocation failed ... No space), green after.

Red and green:
- Model mutants (guest/sim/mutants, crates/fxfs-sim-diff/mutants.sh -c 8): task-149.14.2-growth-keeps-tail (bytes differ at 1000 after a write past a kept tail), task-149.14.2-partial-write-keeps-past-end (same, a write in the kept block past the end), task-164.4-model-seal-forgets-late (new: the seal bug of seed 32826 put back; page's extent open in Fxfs, sealed in the model, after a cut at a seal's end), task-149.29-tail-moved-though-zero (shared blocks differ): all four caught, by the fixed sequences of the_models_past_drifts_from_fxfs and, with PROPTEST shrinking off, by the 64 random sequences alone. Before the generator aimed runs at the last block and seal points at cuts near the end, 64 random sequences missed partial-write-keeps-past-end (twice) and the seal mutant.
- guest/sim's own mutants.sh, 2000 seeds: catches both 149.14.2 mutants (fxfs_growths_zero_past_a_cut); not task-149.29-tail-moved-though-zero, not task-164.4-model-seal-forgets-late.
- In tree: green. Hunt: FXDAX_SIM_DIFF_CASES=1500 FXDAX_SIM_DIFF_OPS=96, passed in 1914 s (interleaved SealCommit 543, SealEnd 553, Copy 1754, Write 461, Reply 474, Commit 523; 3175 failed commits, 10562 kept tails, 3711 remounts). An earlier 3000x96 run died of the tool's timeout at 960 s with nothing found.
- guest/sim in Docker (OrbStack, seccomp off, fxdax-sim image, kernel-tree.sh): mutants.sh --check 33 checked 0 broken; zloop.sh -s 3000: 0 runs failed (1006 s), with the move_tail fix.
- cargo nextest run -p fxfs -p fxfs-posix: 839 passed, 2 skipped (after the fix and its rename).
- scripts/upstream-changes.py --check: passes.
Note: ~/.local/bin/grep on this Mac is a wrapper (eish) that hung mutants.sh non-interactively; the script uses sed.

macOS: FXDAX_TEST_PARTS='workspace scripts' scripts/test.sh --no-fail-fast: 2123 passed, 18 skipped (includes both fxfs-sim-diff tests); scripts part 159 python tests OK, crates/fxfs-sim-diff/mutants.sh --check 4 checked 0 broken. Not done here: AC #3 (it runs in CI) needs the push and a CI run; DoD #1's Linux half (GCE) not run, though guest/sim's Linux zloop passed in Docker.

Commits on task-149-29: c96d6dbb2 (fxfs: dirty room counts the tail a growth zeroes), f86edf9a6 (guest/sim: model leaves a zero tail where it is), 1c7a2487f (crates/fxfs-sim-diff). Left open: AC #3 needs CI on the pushed branch; DoD #1's GCE run.

Rebased on main 10f6159ab (TASK-160.24's moves, TASK-160.32's one mutant set):
- The fix's code now lives in data_object_handle/tail.rs (growth_zeroes_tail, used by txn_grow_size and zero_tail_before_growth) and data_object_handle/write_many.rs (write_many_paid's room). Red again with the room count off (No space), green with it.
- The model's mutants are guest/mutants', a header kind of their own: Kernel: model, Killer: fxfs-sim-diff the_models_past_drifts_from_fxfs (task-149.29-tail-moved-though-zero, task-164.4-model-seal-forgets-late). scripts/guest-mutants-gce.py knows the kind: --check checks the killer names a #[test] of crates/fxfs-sim-diff/src/diff_tests.rs, and that the patch applies; the GCE runner and guest/sim/mutants.sh skip it. scripts/tests/test_guest_mutants.py covers it. crates/fxfs-sim-diff/mutants.sh reads guest/mutants and runs a model mutant against its killer alone; it fails unless each is killed.
- cargo check --workspace --tests --locked: passes with UCD_ZIP set and fails without it, on origin/main and on the branch alike (fxfs-unicode's lib test reads UCD_ZIP at compile time). cargo check -p fxfs-unicode alone fails on flate2's features (no backend selected) on origin/main too. The cc lock bump (1.8.0 -> 1.11.0, find-msvc-tools 0.1.14 -> 0.1.15) is in neither.

After the rebase: cargo nextest run -p fxfs -p fxfs-posix -p fxfs-sim-diff: 842 passed, 2 skipped. crates/fxfs-sim-diff/mutants.sh: task-149.29-tail-moved-though-zero and task-164.4-model-seal-forgets-late killed by fxfs-sim-diff the_models_past_drifts_from_fxfs (12 s each); the two 149.14.2 (Kernel: sim) found too. scripts/upstream-changes.py --check passes. macOS FXDAX_TEST_PARTS='workspace scripts' scripts/test.sh --no-fail-fast: 2135 passed (1 leaky: fxfs super_block::tests::test_mount_with_a_journal_that_starts_anywhere), 18 skipped; 160 python tests OK; mutants --check 17, 4 and 38 checked, 0 broken.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #3: fxfs-sim-diff's the_models_past_drifts_from_fxfs and the_sims_model_does_as_fxfs_does pass in CI 42137709782's workspace job (2303 of 2303), and crates/fxfs-sim-diff/mutants.sh --check (4 checked, 0 broken) in its scripts job; both pass in r691 too, with fxfs-posix's a_writeback_paid_with_dirty_room_has_room_for_the_tail_its_growth_zeroes.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**guest/sim's model of Fxfs is checked against Fxfs by a Rust test that builds the model's C (TASK-149.29, 2026-11-05)**

`crates/fxfs-sim-diff`: its build script compiles `guest/sim/fxfs/fxfs.c` as it is, with `cc`, over a shim of guest/sim's kernel of its own (`shim/`: libc memory, locks that never wait and fail the run if taken held, and kernel/device.c's extent store: lengths, seals, frees, no write to a sealed extent), and a proptest runs sequences of fxfs.h's calls on it and on fxfs-posix's `Fxfs` over a `FakeDevice`, comparing each file after each: size, bytes, every block whole through the extents, holes, sharing, and no extent open in Fxfs that the model says sealed. Operations run at the model's own `sim_point`s, as its locks allow, for the seal drift (seed 32826). It is a workspace crate, so CI's workspace part runs it on Linux and macOS, 64 sequences in about 15 seconds. Decided:
- **The model's C, compiled into a Rust test, not a C driver of fxfs-posix.** fxfs-posix is async Rust with no C ABI outside the guest's fxfs-linux, which needs the kernel's runtime; the model is 840 lines of C needing a dozen kernel calls. *Instead:* a C ABI for fxfs-posix on the host (guest/sim/README.md's "Linking the real Fxfs"), more work, and a C harness without proptest's shrinking. *Undoing it:* delete the crate.
- **A shim of its own, not guest/sim's**, which needs a kernel tree and Linux, so would run only in the guest-sim job. What it copies of kernel/device.c is the store's 60 lines; a drift there shows as a model that fails here alone. *Instead:* split kernel/device.c's store into a file both build. *Undoing it:* that split.
- **Mutants of the model are guest/mutants', a kind of their own** where nothing of guest/sim's kills them: `Kernel: model`, `Killer: fxfs-sim-diff TEST`, a test of the crate's (`scripts/guest-mutants-gce.py --check` checks it names one). guest/sim's `mutants.sh` runs `Kernel: sim` alone, as before; `crates/fxfs-sim-diff/mutants.sh` runs every mutant that changes the model, and fails if one of `Kernel: model` lives; its `--check`, in scripts/test.sh's scripts part, that each applies. *Instead:* `Kernel: sim` and `Killer: none` with a comment, which says nothing kills them, and nothing would run them. *Undoing it:* the kind's two lines in guest-mutants-gce.py and the README's.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
crates/fxfs-sim-diff runs guest/sim's model of Fxfs beside fxfs-posix on random sequences and catches its drifts; it found a model drift (move_tail) and a real Fxfs bug (a dirty-room writeback not paying for the tail its growth zeroes), both fixed. On main: c57fb06ec, c7c3107a8, 0b40eac0a. Verified: in CI 42137709782 (workspace and mutants --check); GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020.
<!-- SECTION:FINAL_SUMMARY:END -->
