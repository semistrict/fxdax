---
id: TASK-336
title: >-
  Framings' write phase takes over a second on a CPU-overcommitted host:
  extentd's framing competes with vCPUs (GitHub runner, 28 vCPUs on 4 CPUs)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-10 23:37'
updated_date: '2026-11-13 15:13'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 737000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-330.1's sandboxes run on GitHub's ubuntu-24.04 runner (4 AMD EPYC vCPUs under Hyper-V, ext4 root mounted nobarrier, data=writeback), short mode: 8 VMs at once, 28 vCPUs, two extent daemons and RustFS on the disk (probe run on branch t330-probe, 2026-11-10). Slowest framings per host 1,159 and 1,292 ms; of the four past 1 s, two are their write phase alone: 'took 1079.1 ms (own 0.0, write 1078.0, sync 1.1, dirs 0.0 ms)' and 'took 1042.3 ms (write 1039.3, sync 2.1)'; the others mostly sync (1,175 ms; 724 ms + write 430 ms). The write phase reads the segment, samples its parts' compression, compresses if it must, and writes the tail: CPU, which the host's vCPU threads take first. On ubu1 with 8 dedicated CPUs (fxjob --bench, same 28 vCPUs) the write phase stays under 0.2 s and slow framings are the syncs (TASK-310's remainder). LangSmith's runners have this shape (8-vCPU VMs on an 8-CPU runner). A framing past a second holds every commit behind it as long. Ideas, unmeasured: run framings at a higher scheduling priority than VM processes (or VMs' threads at a lower), bound framing CPU by sampling less, pin the daemon's framing threads; or show it doesn't matter to commits (they wait on framing's sync, not its write).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 On the sandboxes run's short mode on a 4-CPU GitHub runner, no framing's write phase passes 1 s, shown over several runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Instrument: a framing's write phase split into read, compress and write; every timed thing's threads' waits to run (/proc/thread-self/schedstat), in the warnings, the daemon's stats line and the sandboxes summary.
2. Measure: store-bench framing (stores framing beside spinning threads on few CPUs) and the sandboxes short mode on ubu1 under fxjob --bench.
3. Fix what dominates; A/B it interleaved (ABBA) on the sandboxes short mode for framings, durable lag, I/O stall and the VMs' time; writes ~1.2x, daemon CPU <= 20 ms per guest MiB.
4. Prove: long mode on ubu1 twice clean; GitHub's short mode once.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-13 (claude, TASK-330.5's re-run): GitHub's runner, sandboxes short mode on main 4e0bf82f5 (TASK-369/384/382/388 in; run 43390653591): fails only framings, 2 a host past 1 s (host-a 1,363.0 and 1,264.0 ms, host-b 1,416.3 and 1,357.1 ms), every one of them all write phase (own 0.0, write = total): the syncs no longer show at all. 100/97 framings past the logging threshold, sum 20.8/21.3 s; stores 1.20/1.19x; commands <= 117 ms; daemons 1.73/1.78 GB peak RSS, 33.8/33.3 s CPU; VMs 147.9 s. So this task is now what keeps the short mode out of ci.yml on GitHub's 4-CPU runners.

2026-11-13 (claude, TASK-330.5's second long-mode pass, main 10f613290, ubu1 fxjob --bench, 8 CPUs, 28 vCPUs): the one framing past 1 s (host-b, 1,045.0 ms) was all write phase (own 0.0, sync none), the run's cgroup at up to 65% CPU pressure (some avg10), disk clean of others' writes. Pass 1 (4e0bf82f5) slowest 949 ms, pass 2 1,045 ms: the long mode's framing check now fails on ubu1 by this task alone, as the short mode does on GitHub's runners. This task is what holds TASK-330.5's AC #1 and the short mode's place in ci.yml.

What 'write' covers (segments.rs frame_segment): reading the raw file from cache, sampling or compressing its parts with zstd, and for a copy writing each WRITTEN_BACK piece and waiting for its writeback (write_back), then the tail. The log can't tell CPU (compressing, starved at 65% pressure) from writeback waits (behind the VMs' disk writes) apart: first step here, time those three sub-phases separately (read, pack, write_back), then decide between a framer with CPU of its own / higher priority and fewer writeback waits per framing.

2026-11-13 (claude, TASK-336's agent): instrumented (05bf16471): a framing's write phase is read / compress / write, and every timed thing says its threads' waits to run (runnable, /proc/thread-self/schedstat), in the warnings, the daemon's stats (*_runnable_us, *_sum_*) and the sandboxes summary (slow framings' phases summed, each past the budget). Measured on ubu1 (fxjob --bench): sandboxes short mode on main c211ff96, slow framings read 2.48 s + compress 2.27 s + write 0.56 s of 5.31 s (host-b 2.89 + 2.72 + 0.90 of 6.52 s), 1.13/1.54 s runnable: CPU. store-bench framing (new case): uncontended 93 ms (read 18, compress 32, write 44); beside 28 spinning threads on 8 CPUs 442 ms (read 170, compress 219, write 53; 171 runnable); perf stat: ~8,200 page faults a framing (its fresh 32 MiB buffer), and its packing threads' compressors taken afresh.
Cause of the CPU part: page faults and copying on the framer's thread, and compressors' tables faulted in each framing, each thread getting one thread's share of CPUs 28 vCPUs keep busy. Fix: part buffers (64 MiB) and compressors (64) kept for the framings after (extent_store::framing; 487f443a8, then c6ba0d117). 487f443a8 also read the parts on threads: framings halved, but the VMs paid (sandboxes short ABBA: builders 84.5-120 s against 79.7-88 s, I/O stall 10-35 s against 7-14 s): dead-ends/framing-reads-on-threads.md; c6ba0d117 reads on the framer's thread again. Its ABBA against main before (short9v/9b/10b/10v): slow framings 28-36 against 56-62 a host, sums 2.5-3.1 s against 5.3-6.6 s, slowest 201-288 against 244-423 ms; builders 75.6-80.8 against 80.6-84.6 s; I/O stall 5.7-9.9 against 8.9-13.3 s; durable lag within 50 ms 35-59% against 36-38%; stores 1.17-1.18x. store-bench (pooled, threaded read; the pool is most of it): text beside 28 spinners 421/497 -> 225/265 ms, noise 220/171 -> 27/23 ms.
Not proven: the long mode on ubu1 failed both passes, on what's left, the disk: 487f443a8 (long1): 5 past 1 s (slowest 1.53/1.65 s), every one write phase or read with ~0 runnable, flushes' log syncs taking 2.6-5.3 s meanwhile; c6ba0d117 (long2, ~16:20 UTC): 19 past 1 s (slowest 5.77 s), most the read phase 0.7-5.7 s with ~0 runnable (raw files evicted under the 16 GiB cgroup, read back from the disk: TASK-400), others write phase 1.0-1.5 s (TASK-401); durable lag within 50 ms 36/39%, max 12.4 s. Main before TASK-336 in the same afternoon (longbase): also failed, 11 past 1 s (slowest 2.97 s), 677/701 slow framings (sum 109/114 s; c6ba0d117 456/431, 77/87 s), within 50 ms 41%, max 7.1/7.7 s, daemon CPU 1,137 s (c6ba0d117 1,079 s, 14.5 ms per guest MiB). ubu1's disk degraded through the day (the same base build's short mode: 72% within 50 ms at 12:00, 36-46% by 15:00; host swap full), so morning passes (949/1,045 ms) aren't comparable. GitHub's short mode on 487f443a8 (run 43435575870): 1 a host past 1 s (1,085, 1,549 ms; was 2 a host, 1.26-1.42 s), both write phase with 68-190 ms runnable, slow framings' sums 11.8/11.8 s (were 20.8/21.3 s): TASK-401. Structure: the framer's CPU cost was an accident of buffers and compressors made per framing; kept in one place (framing::Kept) the class goes. Tests: macOS scripts/test.sh green on 487f443a8 (2297, sim, shuttle, scripts after re-anchoring crates/fxdax/mutants/framed-in-copies.patch); ubu1 extent_frame, extent_store, fxdax 832/832, test_cases host 127/127, on each commit; CI 43435481101 green.
<!-- SECTION:NOTES:END -->
