---
id: TASK-111
title: >-
  Sandbox team, round 7 (2026-10-27 report): DAX at half of no DAX at bf22237,
  slowing run over run; benchmark features asked
status: To Do
assignee: []
created_date: '2026-10-28 00:28'
labels: []
dependencies: []
priority: high
ordinal: 414000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team retest at bf22237 (GCE n2-standard-8 us-central1-a, nested, kernel 7.0.0-1011-gcp; Firecracker v1.16.1 + our 25 commits, musl release LTO frame pointers; guest 2 vCPU 2 GiB; one extentd with an 8 GiB budget, RustFS bucket on the host with 25 ms tc netem; postgres:17 fork, initdb, shared_buffers=256MB, pgbench -i -q -s 50, 5 x 60 s -c 4 -j 2; profiling on in every VM). Runs 2026-10-27 21:15-21:43 UTC; no EFAULT, panic or import hang.

tps runs 1-5: DAX 8 GiB 924/862/774/844/807 (mean 842, init 18.8 s; TASK-109.2 fixed: run 3 was 216-472, MAP stays 65-72 us, 4 cache trims 9.3 ms in total); DAX 2 GiB 947/876/814/752/738 (825, 20.1 s); no DAX 1,540/1,654/1,628/1,653/1,631 (1,621, 19.6 s). JuiceFS block rootfs (round 6) averaged 300.

Run 1, DAX 2 GiB against no DAX: commits 36,076 against 57,642; seals 19.3 s total (0.54 ms each) against 18.4 s (0.32 ms); window MAPs 110,533 (evacuate 65,352, restage 34,608, grow 5,283, reach 4,490, range 800), UNMAPs 37,932; extentd commit_seal_s 2.06, stage_s 12.41; VMM extentd client thread user/sys 1.76/16.95 s against 0.82/3.77 (~114 us host kernel per window change over 148,465 changes); fc_vcpu user/sys 80.8/18.3 against 86.7/17.4.

Run over run (run 1 -> 5): 2 GiB guest page faults reaching extentd 7,986 -> 32,532, reach MAPs 4,490 -> 19,311, window full 456 -> 113, commits 36,076 -> 28,761, extentd memory 3.9 -> 7.0 GB; 8 GiB faults 9,557 -> 49,090, reach MAPs 4,320 -> 3,930, window full 0, commits 35,798 -> 32,535, memory 3.8 -> 7.8 GB. extentd time per fault stays ~10 us.

Benchmark features they ask for, so they can stop being our benchmark loop: (1) one CI entry point, e.g. scripts/pgbench-compare.sh --windows none,2G,8G --runs 5 --extentd-memory 8G --bucket-latency 25ms, machine-readable per-run init, tps, latency percentiles and counter deltas, on GCE nested and bare metal; (2) CI thresholds: DAX tps >= no DAX, run 5 >= 90% of run 1, no step change in MAP latency within a VM, one long-lived daemon serving many VMs in a row with its memory full from the start (TASK-108.1); (3) one consistent counter snapshot per phase across extentd, VMM and guest with build commits (e.g. fxdaxctl stats --json); (4) gauges: cache entries pinned by window mappings vs idle (see TASK-72), the window live mapping count, the VMM host mapping count against its budget, the VMM host time per window change (note: already exported as extentd stage-ns, their stage_s); (5) latency distributions for guest fault, seal, evacuation and MAP, not only sums; (6) tracepoints or USDT probes at seal start/end, MAP by reason, evacuation, guest fault to extentd, window reclaim (see TASK-44.16); (7) unstripped vmlinux or System.map per guest build and a nokaslr profiling mode or the KASLR offset in the boot log (see TASK-64); (8) frame pointers and line tables kept in release, symbols not stripped (TASK-92.39); (9) a version stamp from extentd, fxdaxctl, the Firecracker device and the guest module in /metrics and the guest /proc. Evidence on their box: ~/fxdax-evidence/r889-*.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
