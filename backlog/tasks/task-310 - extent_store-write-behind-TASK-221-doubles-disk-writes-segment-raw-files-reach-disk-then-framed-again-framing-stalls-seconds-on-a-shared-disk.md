---
id: TASK-310
title: >-
  extent_store write-behind (TASK-221) doubles disk writes: segment raw files
  reach disk, then framed again; framing stalls seconds on a shared disk
status: To Do
assignee: []
created_date: '2026-11-09 23:57'
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
- [ ] #1 Reproduced on ubu1 (non-ZFS disk, many VMs, one shared disk): extentd's bytes written vs guest bytes, slow framings, measured on main
- [ ] #2 Root cause shown by A/B (write-behind off / 3cb88647e reverted vs main) on the same workload
- [ ] #3 extentd's stats count bytes written per kind (raw, framed, log)
- [ ] #4 Fixed: extentd writes about 1x guest data plus framing overhead on that workload, slow framings back to old-pin levels, and pgbench durable_lag still meets doc-12's window
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
