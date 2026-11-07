---
id: TASK-149.16
title: Hostile arguments in the DAX scripts
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 16:53'
updated_date: '2026-11-07 14:08'
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
- [ ] #1 Scripts write from buffers that fault partway, and from mappings of the same file, same offset and overlapping
- [ ] #2 Scripts stop writes short at unaligned sizes, then go on
- [ ] #3 A write whose source fails is checked against the model at once
- [ ] #4 A short write bug, put back, fails it, shrunk
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
