---
id: TASK-149.16
title: Hostile arguments in the DAX scripts
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 16:53'
updated_date: '2026-11-07 15:34'
labels: []
dependencies:
  - TASK-149.2
parent_task_id: TASK-149
priority: high
type: task
ordinal: 533000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.2 scripts are well-behaved callers: their buffers never fault, sources never overlap targets, and sizes are rarely unaligned at a short stop. The model oracle would have seen the short write bugs had the generator made these inputs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Scripts write from buffers that fault partway, and from mappings of the same file, same offset and overlapping
- [x] #2 Scripts stop writes short at unaligned sizes, then go on
- [x] #3 A write whose source fails is checked against the model at once
- [x] #4 A short write bug, put back, fails it, shrunk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. script.rs: the hostile mix (HOSTILE = WIDE and hostile ops after one op in four): Limit (offsets/lengths past i64's end, negative, of nothing: EINVAL/EFBIG/0 as the VFS checks them), Far (writes and truncations around the DAX window's end, ext4's largest file, a DAX mount's, i64's, on a scratch file; outcome by the filesystem's largest file, script::far), MapOdd, MapFixed (over its own and another file's kept mapping), Mremap (moved), Madvise DONTNEED/REMOVE, TruncateRacing (a child storing below while truncated down and back), FaultingWrite (a PROT_NONE page after good bytes, appends half the time), WriteFromMapping (same offset or overlapping, dst <= src), and ranges cloned/copied onto themselves overlapping. The model says each outcome and errno.
2. script_run.rs: each one done, checked at once around a faulting write or a write from the mapping (check_around), Options::largest per filesystem.
3. differential: a far step's outcome is each filesystem's own, held to the model by its run.
4. fxfs-guest-fsx* switch to HOSTILE; host tests on XFS and ext4 loops, a differential host test.
5. AC4: mutant taking TASK-133's short-write resize away, killed by fxfs-guest-fsx-dax, shrunk in the guest.
6. GCE runs 860-869 (<= 2 hosts): fsx, fsx-dax, fsx-delay-dax (release), fsx-faults-dax (debug), the mutant. Findings fixed test-first; large ones filed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented (cea36e623): script::HOSTILE, fxfs-guest-fsx*'s, the wide mix and a hostile op after one in four: Limit (past i64, negative, of nothing), Far (window end, ext4's largest, a DAX mount's, i64's, on a scratch file; script::far by each run's Options::largest), MapOdd, MapFixed, Mremap (moved), Madvise DONTNEED/REMOVE, TruncateRacing (a child's stores while truncated down and back), FaultingWrite (PROT_NONE after good bytes, appends half the time; check_around at once), WriteFromMapping (same offset or overlapping), ranges onto themselves. A replay of part of a script says each step (Differential::says_steps).
Findings:
1. GCE r860, fxfs-guest-fsx: the reference ext4's, not Fxfs's: delayed allocation left an append from a faulting buffer, stopped short, its blocks reserved past the end; an O_DIRECT read past it failed EIO (iomap_dio_iter's IOMAP_DELALLOC). Reference mounted nodelalloc; a_direct_read_past_an_append_stopped_short_reads_the_file red (EIO) without.
2. GCE r861, fxfs-guest-fsx-faults-dax (no injection needed, r862 on release): the VM went down, AccessRefused: a forked child's store to a page no PRIVATE made the VM's. Fork copied Fxfs's VM_MIXEDMAP PTEs, a large DAX folio's as one, all writable if any was (copy_present_ptes, FPB_MERGE_WRITE). Fixed (9a79bfcab): no VM_MIXEDMAP, as ext4's. cow_fork_after_a_split_pmd; mutant task-149.16-fork-copies-writable-ptes. Diagnosed in r864 and r866 (dax_trace, the child in kvm_async_pf_task_wait_schedule, no Fxfs fault). Filed TASK-149.42 for mremap's and mprotect's batching.
3. Host kernels, not fxdax: XFS's iomap leaves a page's stale bytes past a short copy into a hole (6.18 and 7.2's __iomap_write_begin/_end); host XFS runs skip FaultingWrite. Noted in ~/docs/inbox.
AC4: mutant task-149.16-short-write-keeps-its-size (TASK-133's resize taken away), killer fxfs-guest-fsx-dax.

4. GCE r868 (after the VM_MIXEDMAP fix): fxfs-guest-checks-dax (cow_fork_after_a_split_pmd among them), fxfs-guest-fsx-dax and -delay-dax 300 s each pass. fxfs-guest-fsx failed on the reference ext4, shrunk to an append and a write from the file's mapping 17 bytes behind its source: the model's memmove was mine, the copy is memcpy's, undefined where bytes overlap (the guest's x86 copy wrote a 64-byte line's upper bytes before reading them; host arm64 copied forward). f76c782f2: such writes take the same bytes or bytes apart (the length capped at the distance), the draws unchanged.

5. Mutants (GCE r867, decisions of 2026-11-07): the VM_MIXEDMAP mutant took cow_fork_after_a_split_pmd's run down (AccessRefused), which names no check, so the tool said MISSED: its killer is the suite, fxfs-guest-checks-dax (the user's decision). TASK-133's resize taken away was equivalent since copy-on-write DAX (Fxfs takes a DAX file's size from the inode at writeback; fxfs-guest-fsx-dax passed it): replaced by the real short-write bug, a DAX write stopped short past the end cut back to the size it began at (task-149.16-short-write-cut-to-its-start; the user's decision). r867 also reran fxfs-guest-fsx, -dax and -delay-dax 300 s each with f76c782f2: all pass.

6. GCE r930: both mutants killed: task-149.16-fork-copies-writable-ptes (fxfs-guest-checks-dax, the VM down) and task-149.16-short-write-cut-to-its-start (fxfs-guest-fsx-dax at once, the append's size gone), whose guest shrink kept one append that a replay from a boot passed: the shrink's tries reused files made empty, keeping an earlier try's DAX inode. da35926ce: fxfs-guest-fsx makes its files anew (script_run::made_anew), files_made_anew_are_not_an_earlier_runs. r931: the short-write mutant killed again, shrunk in the guest to 12 of 4082 operations, which a replay from a boot fails the same way. r869: fxfs-guest-fsx-faults-dax (debug kernel) passes, 402 of 870 s.
Validation after the rebase onto 4bde2ca6c: test_cases 110/110 (guest feature, Docker Linux, host XFS and ext4 loops), 101/101 (host feature); macOS scripts/test.sh clippy, workspace 2103/2103, scripts 180; upstream-changes.py --check; guest-mutants-gce.py --check 53, guest/sim mutants.sh --check 42, none broken; guest/sim zloop 500 seeds.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The fsx scripts take hostile arguments (script::HOSTILE, fxfs-guest-fsx*'s): calls past i64's end, negative and of nothing; far writes and truncations by each filesystem's largest file; mmap at odd offsets and sizes, MAP_FIXED over kept mappings, mremap, madvise DONTNEED and REMOVE; truncations racing a child's stores; writes from buffers that fault partway (appends half the time) and from the file's own mapping (the same bytes, or apart), checked at once; ranges onto themselves. Findings: Fxfs's DAX mappings were VM_MIXEDMAP, so a fork copied a large folio's PTEs all writable and a child stored to a page never made private, the VM going down (r861): fixed, 8f7cae3c8, cow_fork_after_a_split_pmd and its mutant. The reference ext4 failed O_DIRECT reads past an append stopped short with delayed allocation (r860): mounted nodelalloc. The model's memmove of overlapping copies (r868) and the shrink's reused DAX inodes (r930) were the harness's: fixed. TASK-149.42 filed for mremap's and mprotect's batching. Verified: GCE r867-r869 (fsx, -dax, -delay-dax, -faults-dax, checks-dax pass), r930-r931 (the short-write and fork mutants killed, the short write shrunk and replayed from a boot); test_cases 110/101; macOS workspace 2103/2103; upstream-changes --check.
<!-- SECTION:FINAL_SUMMARY:END -->
