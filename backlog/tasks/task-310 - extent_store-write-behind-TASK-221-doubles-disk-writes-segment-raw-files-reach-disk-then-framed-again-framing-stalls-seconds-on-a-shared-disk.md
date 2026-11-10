---
id: TASK-310
title: >-
  extent_store write-behind (TASK-221) doubles disk writes: segment raw files
  reach disk, then framed again; framing stalls seconds on a shared disk
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 23:57'
updated_date: '2026-11-10 16:04'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 718000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reported by LangSmith (sandbox CI, 8-vCPU runner, two sandbox hosts each with its own extentd, one local RustFS bucket on the same disk), after the bump 6a513509a -> 96c7808b8. Their fxdax E2E job became flaky; the non-fxdax job did not.

Per CI run, from extentd's 'extent store: framing ... took' warnings (logged above 50 ms):
- Old pin: 33-51 slow framings, slowest 0.2 s, 3-4 s total, about a third in the write phase.
- New pin: 940-1,065 slow framings, slowest 3.3-9.6 s, 245-314 s total, about 85% in the write phase. 'commits wait for object storage' also hit the 1 GiB lag bound.
- Sandboxes: /bin/echo took 1.9 s, uname 6.3 s, cat 11.9 s; creates took up to 43 s; tests timed out.

Their controlled workload: 16-vCPU GCE VM with a persistent disk, new pin. 16 sandboxes, 8 at a time, each writing 768 MiB with fsync, then 3,000 files of 8 KiB, reading back and deleting. That is about 12.4 GiB of guest writes in about 130 s.
- extentd wrote 28.8-29.0 GiB, about 2.3x the guest data, with cancelled_write_bytes = 0.
- The disk wrote 42.7-45.0 GiB in all, including RustFS's copy.
- 104-135 slow framings per run, up to 17.5 s.
- I/O stalls for 35-38 s of the 130 s.

Suspected cause, TASK-221's commits:
- 6e5d8b0db writes the raw segment back a MiB at a time as the guest writes.
- bed99ce0c makes writes wait once 8 MiB of write-back is in flight.
- 3cb88647e has a flush sync closed segments' raw files again, reverting 981d28456.
Before these, a raw file mostly stayed in the page cache and was deleted once framed. Now every byte reaches the disk twice, raw and framed, and framing's write phase competes with raw write-back. TASK-221 was tuned on one VM with a dedicated disk (pgbench).

Not yet verified by them:
- An A/B with write-behind returning None and 3cb88647e reverted.
- The old-pin baseline: the old kernel refuses version 63.

Fix direction, theirs:
- Skip or defer raw write-back for segments that will be framed soon.
- Drop the raw file's pages once framed.
- Make write-behind adaptive.
- Count extentd's bytes written per kind (raw, framed, log) in the stats.

Whatever the fix, it must keep doc-12's durability window (TASK-221's ACs).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Reproduced on ubu1 (non-ZFS disk, many VMs, one shared disk): extentd's bytes written vs guest bytes, slow framings, measured on main
- [x] #2 Root cause shown by A/B (write-behind off / 3cb88647e reverted vs main) on the same workload
- [x] #3 extentd's stats count bytes written per kind (raw, framed, log)
- [ ] #4 Fixed: extentd writes about 1x guest data plus framing overhead on that workload, slow framings back to old-pin levels, and pgbench durable_lag still meets doc-12's window
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store counts the bytes it writes per kind (raw: guest writes to segments' raw files; copied: sealing's copies into the open segment; framed; own: extents' own files; log: logs and WAL base); extentd's daemon stats line says them, with its /proc/self/io write_bytes and cancelled_write_bytes.
2. A guest case, fxfs-guest-shared-disk (manual): its own daemon, stores, file bucket and part cache under the case's directory, on one disk; VMs N at a time, each writing a large file with fsync and many small files, reading back, deleting; the host says extentd's bytes written (proc io, per kind) against the guests', slow framings, and the disk's stall time (PSI io, device stat).
3. ubu1: a system-setup.d step making an ext4 scratch filesystem on a ZFS zvol, the case's directory on it.
4. Measure main vs write-behind off + 3cb88647e reverted (A/B).
5. Fix from what the A/B shows; measure again; pgbench and pgbench-dax durable_lag within doc-12's window.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-09 (claude): branch t3/ed8c1326 (b2c9a5421): extent_store::written() counts bytes stores write by kind (raw, copied, framed, own, log); extentd's daemon stats line has written_*_bytes and io_write_bytes / io_cancelled_write_bytes (/proc/self/io); keeps_object_storage_and_follower_stats green on ubu1. Case fxfs-guest-shared-disk[-dax] (manual): its own daemon, stores, file bucket and part cache on one disk; VMs N at a time, each 768 MiB fsynced + 3,000 x 8 KiB files, read back, deleted; says guest vs extentd bytes, by kind, slow framings, PSI io stall, disk writes. ubu1: ~/docs/system-setup.d/143-fxdax_ext4_scratch.sh (rpool/fxdax-scratch zvol, ext4, /mnt/fxdax-scratch). A/B side: branch t310-ab-off (write-behind off; flush waits for framings of segments closed with data since the commit before, 3cb88647e reverted by hand onto TASK-301's flushes). Artifacts built on ubu1 (~/src/t310-arts-main, ~/src/t310-arts-ab, kernel ~/src/t310-kernel). Blocked: Tailscale SSH to ubu1 wants the user's re-authentication check.

2026-11-10 A/B on ubu1 (ext4 on a zvol, /mnt/fxdax-scratch; fxfs-guest-shared-disk, 16 VMs 8 at a time, each 768 MiB fsynced + 3,000 x 8 KiB, no DAX; host shared with other agents, load 40-80, so latencies are noisy, bytes are not):
- main (2 rounds): extentd write_bytes 38.97 / 39.34 GiB for 12.37 GiB of guest data (3.15x / 3.18x, the file bucket's copy included), cancelled 0.00-0.01; by kind: raw 12.49, own 10.55, framed 2.02-2.30, copied 1.03-1.22, log 0.01, the rest (bucket and part cache) ~12.7. Slow framings 69-72, slowest 11.9-17.1 s, sum 122-197 s; slow flushes 89-111; durable lag max 11.4-15.3 s; I/O stall (PSI some) 61-77 s of 167-178 s.
- write-behind off + flushes waiting for framings (t310-ab-off, 1 round): 39.13 GiB (3.16x), cancelled 0.20; raw 12.49, own 10.54, framed 2.02, copied 1.39; slow framings 94, slowest 8.1 s, sum 71 s; durable lag max 8.3 s; stall 47 s of 197 s.
Bytes: the same with write-behind off. Root cause of the bytes: the large file's extents are large (first write past large_extent), written to open extents' own raw files (raw 12.49), which the kernel writes back as the guests write (memory and dirty thresholds, 8 guests at once), then sealed by framing a copy into extents/ (own 10.55): every byte twice. Write-behind isn't involved for them (it writes back segments' raw files only). Fix (5a0a944b9): a large extent none of whose parts compresses is framed in place, its raw file its own file, its tail appended, renamed into extents/.

2026-11-10, isolated (fxjob --bench: cores 4-7 the run's alone, --mem 20G; the disk shared: nvme0n1 wrote 39-86 GB in all during each run, ~half other jobs'). base = main with this task's counters and case (t310-base); fix = 2e16eff5c's code; ab = fix with write-behind off (t310-ab-off).
- fxfs-guest-shared-disk, 16 VMs 8 at a time: base 39.25 GiB written (3.17x; stores raw 12.49 + own 10.56 + framed 2.26 + copied 1.01, bucket and cache 12.93), 145 s, slow framings 64, slowest 14.1 s, durable lag max 14.3 s, PSI full 27 s, disk busy 121 s. fix 31.54 GiB (2.55x; own 1.00, raw 12.49, framed 2.89, copied 1.57, bucket and cache 13.58), 107 s, slow framings 69, slowest 11.1 s, durable max 9.2 s, PSI full 33 s, disk busy 93 s. Unisolated rounds (load 10-30): base 40.3-40.8 GiB, fix 31.6-32.4 GiB.
- pgbench / pgbench-dax, durable_lag within 50 ms, p99, max: base 62%/68%, 256/256 ms, 4.3/8.9 s (another round at 20G: 64%/72%, 4.1/0.26 s p99, 7.1/57 s max, others' disk busy); fix 82%/77%, 128/128 ms, 0.49/2.55 s; ab (write-behind off) 62%/60%, 256/512 ms, 5.0/5.2 s. Bytes: io_write ~36-38.6 GB per case on every side; cancelled 0.16-4 MB base, 120-270 MB fix, 790-890 MB ab.
- So write-behind costs no bytes (raw reaches the disk with it off too, by the flushes' syncs and the host's write-back) and keeps the durability window: it stays. The triple write (raw, log, framed for small commits) is not material: logs 0.2 GB of 36 GB under pgbench (runs held in records), 0.02 GiB in the shared-disk case with each small file fsynced. pgbench's raw is 27-29 GB for 1.9-2.0 GB framed: open extents rewritten append to the open segment, dead before framing (TASK-92.41's interleaved/rewritten kinds), written back regardless.
- doc-12's window does not hold on this shared disk: the crash bound (1 s) is passed by pgbench's max lag on base and by -dax on fix, and the fsync-per-file shared-disk case fails the median on base and fix alike (TASK-322).

2026-11-10: on main: 5949ac1f2 (written-bytes stats; fxfs-guest-shared-disk), 90b4d8943 (TASK-329, found by the crash rounds once extents are framed in place), 80e682ed5 (spill once a segment is set aside; large extents that don't compress framed in place), 3a1038195 (backlog). Verified: macOS workspace (2216), sim, Shuttle; ubu1 workspace (2470, target and TMPDIR on ext4), sim, Shuttle; pgbench and pgbench-dax under fxjob --bench meet doc-12's median (82%, 77%). AC #4 in part: the stores write 1.47 times the guests' data on the shared-disk case (2.17 before), 2.55 with the file bucket's copy (3.17); not ~1x: the segments' raw data, a write-ahead log, is still written back and then framed (inherent to raw + framed; framing segments in place, as large extents now are, would remove it when they don't compress), and the spilled extents are a file each, whose syncs (flush data phase, framing own phase) leave the slowest framing at ~11 s with 8 VMs at once, not the old pin's 0.2 s: TASK-92.41's commits spanning segments removes the file per extent. pgbench-dax's longest lag (2.55 s) passes the crash bound on this shared disk, as main's did (up to 8.9 s): TASK-322. ubu1 setup: ~/docs/system-setup.d/143-fxdax_ext4_scratch.sh (zvol rpool/fxdax-scratch, ext4 at /mnt/fxdax-scratch, primarycache=metadata).

CI 42843850345 green at 3a1038195 (all jobs).

2026-11-10 (claude, TASK-92.41's agent): finishing AC #4 by TASK-92.41 (commits spanning segments, replacing the spill) and segments framed in place when they don't compress.
<!-- SECTION:NOTES:END -->
