---
id: TASK-310
title: >-
  extent_store write-behind (TASK-221) doubles disk writes: segment raw files
  reach disk, then framed again; framing stalls seconds on a shared disk
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 23:57'
updated_date: '2026-11-11 22:52'
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
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store counts the bytes it writes per kind (raw: guest writes to segments' raw files; copied: sealing's copies into the open segment; framed; own: extents' own files; log: logs and WAL base); extentd's daemon stats line says them, with its /proc/self/io write_bytes and cancelled_write_bytes.
2. A guest case, fxfs-guest-shared-disk (manual): its own daemon, stores, file bucket and part cache under the case's directory, on one disk; VMs N at a time, each writing a large file with fsync and many small files, reading back, deleting; the host says extentd's bytes written (proc io, per kind) against the guests', slow framings, and the disk's stall time (PSI io, device stat).
3. ubu1: a system-setup.d step making an ext4 scratch filesystem on a ZFS zvol, the case's directory on it.
4. Measure main vs write-behind off + 3cb88647e reverted (A/B).
5. Fix from what the A/B shows; measure again; pgbench and pgbench-dax durable_lag within doc-12's window.

6. (2026-11-10, claude, TASK-310 finish) Trace which syncs the slow framings wait on (scripts/bpftrace/store-syncs.bt on ubu1's shared-disk case); take them off the framer: a framing writes its framed file and ends once a per-store thread made it durable with the segments framed meanwhile (one sync of their files at once, one of their directory); drop the syncs nothing needs (the framings' and flushes' of the directory of framed files, the raw-file removals'); recovery made not to rely on framings being durable one after another; deterministic crash tests for the new orders; interleaved A/B on ubu1 (shared-disk, -dax, pgbench and -dax 6 rounds, TASK-322's fsync case).
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

2026-11-10 (claude, TASK-92.41's agent): AC #4 by TASK-92.41 (commits spanning segments, replacing the spill) and segments framed in place unless compressing saves an eighth (their parts raw, the raw file linked into segments/, its data written once; in-place decided by a sample of one part in eight). Interleaved A/B on ubu1 (ext4 scratch zvol; each run fxjob --bench; main 0a8c2e27b's base 41e13503f vs branch 29eb9131e), 3 rounds a side:
- fxfs-guest-shared-disk (16 VMs, 8 at once, 768 MiB fsynced + 3,000 x 8 KiB, file bucket on the disk): stores' own writes main 17.0-18.3 GiB (raw 12.49, copied 1.56-1.57, framed 2.53-3.08, own 0.64-1.19), branch 12.67-12.68 GiB (raw 12.49, copied 0.17-0.20, framed 0.00-0.03, own 0): 1.37-1.48x vs 1.02-1.03x the guests' 12.37 GiB; extentd in all 2.46-2.60x vs 2.05x (the rest the bucket's copy and the part cache). Slowest framing 17.3-36.0 s vs 4.6-8.1 s; durable lag p99 32 s vs 8-16 s, max 17-35 s vs 8-11 s; within 50 ms 155-185 vs 159-169 of ~273; secs 126-137 vs 100-118; segments framed in place 158-160 of ~292; 16 commits spanning segments per run.
- fxfs-guest-shared-disk-dax (first measured: TASK-317's bound holds, no OOM on either side, extentd memory 20 GiB cap): main 2.46-2.52x, stores 17.1-17.4 GiB, slowest framing 13.2-26.0 s; branch 2.05x, stores 12.66-12.67 GiB, slowest framing 2.0-4.7 s; p99 16-32 s vs 4-8 s.
- The slowest framings left are their file and directory syncs (fdatasync and the directories' fsyncs, 2-4 s each), the disk's queue: io pressure 'some' 22-42 s of the runs; the write phase (read, sample, tail) is 0.1-0.5 s. The target of a slowest framing under 1 s is not met on this shared disk; the old pin's 0.2 s was not on one.
- fxfs-guest-pgbench, 6 rounds a side: within 50 ms main 57.2-68.9% (mean 62.6), branch 57.3-63.5% (59.9); p99 256 ms both (one main round 512); max 1.9-4.0 s vs 1.3-3.7 s; tps ~3,060 both; copied 4.9-5.4 GB vs 4.4-4.9 GB, own 39-63 MB vs 0. -dax: 57.9-70.1% (62.0) vs 58.3-65.9% (61.3); max 1.1-3.2 s vs 0.9-3.0 s. No difference past the rounds' spread; doc-12's median kept on both. pgbench's segments compress: none framed in place.
- TASK-322 (FXDAX_SHARED_DISK_FSYNC=1, 3 rounds a side): durable lag p50 8-16 s on main vs 0.26-2 s on the branch, p99 16-32 s vs 8-16 s, within 50 ms 1.4-3.5k vs 0.7-10k of 48.3k; stores 1.37x vs 1.02x. It moves, a lot, but doc-12's 50 ms median still breaks on this disk.
- Found: TASK-332 (main's spill holds two descriptors per spilled extent: store-bench burst fails at ulimit 1024); gone with the spill.
Linux (ubu1): workspace 2479 passed, sim 46, shuttle 37+10+6+67+10+18; macOS: workspace 2225, clippy, sim, shuttle green.

AC #4, where it stands (main ba0c184d2, CI 42900426848 green): bytes met (stores 1.02-1.03x the guest data plus framing tails, against 1.37-1.48x; 2.05x with the bucket's copy against 2.46-2.60x); pgbench and pgbench-dax keep doc-12's median, no difference past the rounds' spread; slow framings NOT back to the old pin's 0.2 s: 2.0-8.1 s on this shared disk (main 13-36 s), all of it the framings' file and directory syncs behind the disk's queue, as TASK-322's commits wait behind it. Left open for that: what's left is the shared disk's queue, not bytes written twice.

2026-11-10 (claude, finishing AC #4's framings): which syncs. scripts/bpftrace/store-syncs.bt (fexit ext4_sync_file, each extentd sync by whose file, kind, thread; jbd2 commits) on main 7e1740cb7's fxfs-guest-shared-disk, ubu1 ext4-on-zvol scratch, fxjob --bench (main-trace-2, box quieter than the 92.41 agent's): each framing made three syncs one after another on its framer, its framed file (fdatasync; for one framed in place its raw file, linked), the directory of framed files and the directory of raw files, 0.3-0.75 s each in the burst at 34-40 s (one framer: file 663 ms, then segments/ 747 ms, then open-segments/ 680 ms), the next framing of the store waiting behind them; the flushes' raw-file and log syncs up to 0.7 s; and the slowest syncs of all, 1.1-1.4 s, a new log's header synced under the state lock by a write filling the open segment (filed: TASK-333). Each sync is a jbd2 commit and a cache flush of the zvol, which waits for the zvol's queued writes (eight VMs' and the file bucket's). Two of the three framing syncs were needed by nothing: the directory of raw files changes as no segment is framed, and remove_raw re-synced the directory of framed files the framing had already synced; but recovery leaned on both side effects (below).

2026-11-10 (claude): the change. A framing reads, frames and writes its segment, then a thread of the store's makes the segments framed meanwhile durable together (segments::make_durable: their files synced at once, then segments/ once, its own timings: durable_* in the stats, 'making ... segments framed durable took' warnings); the framing ends only then, so the uploader, close_now and close wait as before. Gone: the framings' open-segments/ sync (a framing syncs open-segments/ and logs/ first only if its segment's entries weren't synced since it opened: Segments::take_unsynced_entries), the flushes' segments/ sync, remove_raw's segments/ sync. Bugs the move exposed, each a blind spot (all found by the crash tests once the order changed; each test red without its fix): (1) recovery took any whole framed file as its segment's record, safe only while each framing was durable before the next was written; a later one kept whole by a crash over an earlier one lost read commits past what the earlier lost (store test segments_made_durable_together_survive_crashes_making_them_durable: a commit spanning segments, its ender whole, its first segment torn; Shuttle crash round 137: any later footer). Fixed in wal::recover: a whole framed file is trusted only up to the first segment not whole (but for leftovers of segments the base acknowledged, or whose raw file or log is gone, durable then); after it, segments whose raw file and log are both there are closed again from them (wal tests segments_framed_after_one_lost_close_again_from_their_logs, segments_acknowledged_brought_back_break_no_whole_run). A framer holding back spanning commits' enders was tried first (dead-ends/framer-orders-spanning-commit-enders.md). (2) recovery trusted a framed file unchecked when its raw file and log were absent, safe only because the framing's open-segments/ sync had made the next segments' entries durable as a side effect; a framed file whose raw file's entry was never durable was read torn (same store test, op 21 seed 14): hence take_unsynced_entries. (3) TASK-334 (latent on main): a log record kept after an earlier one lost, or refused by its runs check, kept a commit not durable. Structure: recovery's assumptions about orders were implicit, kept by side effects of syncs that looked redundant; they are explicit rules in wal::recover now, each with a deterministic test. No framing waits for another's syncs.

2026-11-11 (claude): final A/B on ubu1 (main cf757b22e, with TASK-341/345, vs this branch; one guest kernel built from main; fxjob --bench, interleaved; 3 rounds of the shared-disk cases, 6 of pgbench). Slowest framing, median (range): shared-disk 564 (438-918) vs main 964 (805-1042); shared-disk-dax 460 (448-647) vs 742 (464-1844); shared-disk fsync 470 (388-1057) vs 1501 (1459-1560); pgbench 129 (104-180) vs 740 (186-5521); pgbench-dax 145 (110-171) vs 1068 (178-1431). 35 of 36 branch runs under 1 s (the one at 1.06 s, fsync round 1, under other agents' load). What's left of a slow framing is its write phase alone (own 0.0 ms, write 0.4-0.6 s on the shared disk: reading, compressing, writing on a contended disk; TASK-336); no sync. Durable lag (TASK-322): median unchanged everywhere (p50 16-32 ms shared-disk, 128 ms fsync on both sides: doc-12's 50 ms median still broken there, TASK-322; 64 ms pgbench on both); within 50 ms shared-disk 75.7% vs 67.3%, fsync 22.1% vs 20.7%; p99 shared-disk 1024 vs 2048. Regression: pgbench-dax's tail (p99 1152 vs 256 ms, slowest flush 4.6 s vs 1.4 s median), filed TASK-369. Every pgbench run passed its own durability check on both sides. Bytes: extentd wrote 25.4 GiB on both sides of shared-disk (raw 1.0, framed 0.002-0.005 per guest byte). One branch shared-disk round (2) hung 2.5 h: fxdax.slice at its 18 GiB limit with another agent's job (t358-b, OOM-killed at 13:37), the daemon paging in from its cache at 1.6 s a read; not counted as a hang of this code. The uploader's landing of a spanning commit's first segment last (TASK-338's first fix) is dropped: TASK-345's rebase follows such a segment landed alone; the acknowledgement hold stays. Tests: macOS scripts/test.sh green; Linux workspace on ubu1 2502/2503 (TASK-315 alone); crash tests 10/10.
<!-- SECTION:NOTES:END -->
