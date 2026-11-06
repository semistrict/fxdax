---
id: TASK-44.15
title: >-
  Random file operations with an oracle on the guest Fxfs: fsx-style mmap,
  write, truncate and fsync, differential against ext4, xfstests' generic group
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-24 21:31'
updated_date: '2026-11-06 09:16'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 1500
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest checks are each a scenario someone thought of. fsx's random mixes of mmap stores, writes, truncates and syncs, verifying every byte, and the same sequence run on ext4 to compare, find interleavings no one listed; xfstests' generic group is the kernel's own suite for such bugs. Under fault and delay injection they reach the DAX paths' rare branches.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest test runs seeded fsx-style operation sequences on Fxfs with and without DAX, verifying contents, and the same on ext4 to compare
- [x] #2 xfstests' generic group runs against the guest Fxfs, its failures triaged and filed
- [ ] #3 Both run with injection on (the fault and delay injection tasks)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. script.rs: a wide mix (WIDE) of new operations, drawn only when Mix::wide, so random_ops' and fxfs-guest-script's scripts stay as they were: ranges cloned (FICLONERANGE) and copied (copy_file_range), ranges zeroed and allocated (fallocate), msyncs, fdatasyncs, stores through fresh shared and private mappings, O_DIRECT reads and writes, a file's page cache dropped, a file closed, evicted and reopened (which makes a DAX mount's file DAX, copying on write, where one kept open stays in the page cache). The model predicts every operation's outcome, its errno too: the VFS's remap and copy checks (generic_remap_file_range_prep, generic_copy_file_checks) as a pure function, checked against XFS's reflinks by the host-side tests.
2. script_run.rs: each step's outcome (a count or an errno) and which of its files' mtime and ctime it changed, recorded; the interpreter runs the new operations.
3. A differential run: the same script on Fxfs and on ext4 in one guest, step by step, ext4 first. ext4 failing the model is the model's mistake (its own kind); Fxfs failing it, ext4 passing, is Fxfs's; the timestamps each step changed are compared between the two, the model having none, with the differences allowed listed and why. ext4 has no reflinks: a clone on it is a copy of what the model says the clone takes.
4. Cases fxfs-guest-fsx and -dax (and -delay-dax, fxfs.dax_delay_us): the host formats ext4 on a virtio-blk image; the guest runs scripts seed after seed for FXDAX_FSX_SECS, saying each seed; a failure is shrunk in the guest (cheap, no boot a try), its drops said and replayed once from boot by the host to confirm; a guest that died is shrunk by the host's VM replays (script_vm), as fxfs-guest-script's. Tagged into the suites.
5. Fault injection: fxfs-guest-fsx-faults-dax on the debug kernel, failslab and fail_page_alloc for the Fxfs half's calls alone, ENOMEM a refusal whose effects are checked within what the operation could have reached, ext4 brought to what Fxfs holds after one.
6. Tests: script.rs's model rules and the generator (host, any OS); script_run's on the host's ext4 and XFS loops (Linux, root); the harness pieces. Compile in Docker (OrbStack, /target/t4415); GCE r693-r699: fxfs-guest-fsx and -dax jailed 10 min each, delay and faults, xfstests generic/quick both; bugs found fixed with pinned tests or filed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
random_ops (guest checks, DAX and not): 3000 seeded operations against an in-memory model of the file, verified each read and every 256th op whole; passes on GCE r82 (kernel b8). The model is the oracle in place of ext4 (the checks VM mounts Fxfs alone); xfstests' generic group and runs under injection left.

2026-10-31: xfstests-fxfs-guest(-dax): generic/quick on Fxfs, as virtiofs's tags (test_xfstests.rs). First run, r342: non-DAX stopped by a panic at generic/161 (60 passed, 51 not run), DAX by a soft lockup at generic/111 (37, 40). Many mismatches are the guest's, as it has no /dev/fd.

2026-11-05 (task-44-15): the wide mix, its model, the differential runner and the cases written; Linux (Docker via OrbStack, /target/t4415): test_cases --features guest 88/88, --features host 77/77, runner 4/4, the guest's musl check clean. What the tests found on the way, of the harness: an empty write in the model grew a file to its offset (now a no-op); copy_file_range looped to its length copies, within a file it grows, what it just copied (the loop now stops at what the source held as it began); an O_DIRECT write refused for room (ext4, full) had written some of its blocks: iomap_dio_complete returns the error, not the count, so a refused direct write is held to each byte old or new, a buffered one to nothing changed. Times compared only in the guest: a host's filesystem may keep them coarse (two runs on one overlay disagreed on punches in one tick). GCE: the project's 32-CPU quota was full (eight other sessions' hosts), r693-r695's creates refused; the three fsx cases run on one host, r693, once a slot frees.

GCE r693 (us-west1-b, release kernel, jailed, FXDAX_FSX_SECS=600): fxfs-guest-fsx 49 scripts, -dax 39 (3,610 files read in DAX), -delay-dax 37 (3,408): all pass, no time differences. r694 (us-east1-c, debug kernel: lockdep, KCSAN, fault injection): fxfs-guest-fsx-dax 18 scripts, 1,712 files DAX, 49,728 time changes agreed, pass; fxfs-guest-fsx-faults-dax FAILED, finding 1: fallocate(ZERO_RANGE|KEEP_SIZE) returned EIO under failed allocations, 'fxfs: EIO (errno 5): the extent store failed with errno 12': the guest device's errno_error made any errno but ENOENT, ENOSPC and EROFS a bare DeviceError, which the errno table has as EIO. Fixed: fxfs_posix::device_error passes ENOMEM through (device.rs uses it); a_device_out_of_memory_is_enomem red without (EIO), green with. Blind spot: nothing failed the device's allocations with a caller's errno checked (fxfs-guest-faults-dax accepts any errno). r695/r696 (us-central1-b, us-west1-b): xfstests generic/quick with DAX and without pass, 650 listed, 365/363 not run, the same 13 known failures; generic/225, 564 and 798 now pass on both and are out of KNOWN_FAILURES. r697 (us-east1-c, debug kernel, the fix): fxfs-guest-faults-dax passes; fxfs-guest-fsx-faults-dax FAILED, finding 2: an O_DIRECT read of file 0 differs from the model from 0x1fe4f6, where a clone of file 2 onto file 0 ended, file 2 having had a truncate refused for memory (and 'mapping zeros past ino 5's end ... failed: -12' in the log): a source's last block holding bytes past its end that a clone, taking it whole, carries. Not replayed: fault injection isn't seeded. Failures now say what changed the byte that differs, back through clones and copies (Run::lineage), for r698.

r698 (us-central1-b, debug kernel, both fixes but the write's): fxfs-guest-fsx-faults-dax FAILED, finding 3: write(2) EIO after 'making 1 pages at 178000 of the window private: -12': fxfs_dax_dirty made any failed PRIVATE -EIO, the device's ENOMEM too. Fixed: ENOMEM stays ENOMEM (dax.c), as fxfs_dax_throttle's no room already is; guest/sim's device fails a request with an errno a test says (sim_device_fail_with), and cow_private_failed's write whose PRIVATE had no memory is ENOMEM: red without (-5), green with; scenarios, zloop -s 300, mutants.sh --check (34; task-164.4-writeback-unowned and task-164.9-private-failure-ignored rebased onto the change, both still killed) and guest-mutants-gce.py --check (38) pass. r699 (us-central1-a, us-west1 out of quota; debug kernel, all fixes): fxfs-guest-checks-dax pass, fxfs-guest-fsx-dax 26 scripts, 2,480 files DAX, pass; fxfs-guest-fsx-faults-dax FAILED again, finding 2's class, now with its lineage: a clone within file 0 carried what its source held before op 528's write, an fsync between refused ENOMEM: fxfs_dax_write_back leaves a failed writeback's pages clean. Filed TASK-207 (high): it changes doc-13's writeback on errors, and r697's mismatch is likely the same. xfstests: AC 2 met by r695/r696, the 13 failures each a known one with its reason and task (TASK-143 to 145, the rest said in KNOWN_FAILURES). AC 3 open: the fsx cases run under delay injection (r693) and fault injection (r694, r697-r699), xfstests under neither, and -faults-dax fails until TASK-207.

Every-bug notes (AGENTS.md). Found by: the fsx cases under the debug kernel's fault injection, a refusal allowed only for ENOMEM, its effects held within what the operation could reach. Blind spot: fxfs-guest-faults-dax let any errno fail a call, and checked only fsynced files after; no test failed the device's requests for memory and looked at the errno or at the data written before a failed writeback. Class tests: the fsx-faults case (any call's errno and the data after), sim_device_fail_with (any device errno in guest/sim). Looked around: the other -EIO in dax.c are the protocol's own (seals that never end, a write finding no range, a range past its file), not a device's errno; device.rs's errno_error passed ENOENT, ENOSPC and EROFS and nothing else, now ENOMEM through fxfs_posix::device_error. Structure: a device's errno and what a caller sees of it are decided in one function now, the rest EIO by the table's rule; the writeback's error semantics are the structural question, TASK-207.
<!-- SECTION:NOTES:END -->
