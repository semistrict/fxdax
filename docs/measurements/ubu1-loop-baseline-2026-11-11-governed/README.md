# After TASK-359 and TASK-358: the cache fixed, the daemon's memory a page cache

ubu1, 2026-11-11, 15:16–15:47Z, tree 1e5367ef. The same three interleaved rounds as
[the first run](../ubu1-loop-baseline-2026-11-11/README.md), with two changes on the fxdax side:

- TASK-359: the host's part cache sits on its stores' filesystem (test-e2e.sh), and a store
  elsewhere is refused. Before, every acknowledged segment's move into the cache failed (EXDEV)
  and reads past memory went to the bucket.
- TASK-358: no fixed daemon memory (`FXDAX_EXTENTD_MEMORY` unset): the daemon takes what the host
  and the run's cgroup (`fxjob --mem 16G`, both sides) have free, less a headroom (here 1.7 GiB, a
  sixteenth of 27 GiB).

Working sets are still sized by 1 GiB (the old fixed memory). Load 4.3–16.0 (runs.tsv): round 2's
fxdax run ended at 15.96, just under the bound.

## Before and after

| Workload | ext4-loop-unsafe | fxdax before | fxdax after | fxdax vs baseline, after |
|---|---|---|---|---|
| ws-2x-rand-read (2 GiB) | 134,800 IOPS | 66 IOPS | 659,600 IOPS | 0.20× (fxdax 5× faster) |
| ws-4x-rand-read (4 GiB) | 30,430 IOPS | 43 IOPS | 351,800 IOPS (522 in round 2) | 0.09× |
| ws-1x-rand-read (1 GiB) | 699,200 IOPS | 19,250 IOPS | 675,600 IOPS | 1.03× (same) |
| rand-read-cold | 54,250 IOPS | 4,223 IOPS | 22,790 IOPS | 2.4× |
| sqlite-pread-scan (cold) | 1,546 MiB/s | 257 MiB/s | 387 MiB/s | 4.0× |
| read-small-cold | 34,170/s | 16,780/s | 36,320/s | 0.94× (same) |
| seq-read-cold | 3,710 MiB/s | 1,552 MiB/s | 1,463 MiB/s | 2.5× |
| pgbench | 5,258 tps | 3,482 tps | 3,221 tps | 1.6× |
| fsync-append | 11,250 IOPS | 6,077 IOPS | 5,550 IOPS | 2.0× |
| sync / syncfs / freeze loops | 6,059 / 26,330 / 1,591 /s | 4.2 / 5.2 / 3.8 /s | 3.2 / 3.9 / 3.1 /s | 1,900× / 6,700× / 510× |

The bucket: ranged reads per fxdax run fell from 5,025 to 1–5 (extentd-stats.txt per round).

## Left

- **sync, syncfs, freeze** (~250–330 ms each): unchanged; each waits for the bucket (upload and
  lease), which production's JuiceFS with flushes dropped never does. Not addressed here.
- **Cold reads inside memory** still lose 2.4–4×, sequential 2.5×: DAX faults through the daemon.
- **ws-4x round 2, 522 IOPS**: the host's load reached 16 during it; the governor gave memory back
  as other jobs took it, and misses decompressed a 1 MiB part each from the local cache (no
  bucket reads). A run at a quiet moment would tell whether that is the governor reacting
  correctly to real pressure or giving back too much.
- pgbench and the fsync/metadata workloads: 1.6–2.2×, unchanged.
