# fxdax against JuiceFS's best case, stood in for by a local loop device (TASK-350)

ubu1, 2026-11-11, 14:25–14:42Z. Three rounds, the cases' order alternating (scripts/bench-rounds.sh),
each run `fxjob --bench` (cores 4-7), host page cache dropped before each; 1-min load 2.9–3.8
throughout (runs.tsv). Tree 5dd0015d (main), guest kernel built from it (~/src/t350-kernel).

- **ext4-loop-unsafe** (stand-in for production JuiceFS with everything in its local cache):
  ext4 in the guest on a virtio-blk disk, `cache_type` Unsafe as production runs, over
  /dev/loopN (buffered, no direct I/O) over a raw image on ZFS (/home).
- **fxdax-dax**: the guest's Fxfs through a DAX window, extent daemon of 1 GiB
  (`FXDAX_EXTENTD_MEMORY=1073741824`, the production default), file bucket on ZFS with
  `FXDAX_REMOTE_LATENCY_MS=25`, part cache on disk.

Both VMs: 2 vCPUs, 2 GiB. One iteration a round of the fs bench's groups io, metadata,
working-set, sync and pgbench (20 s, scale 10). `ab.md` is scripts/bench-ab.py's table; the JSON
per run is beside it. Above 1 in "vs", fxdax is slower.

## Where fxdax loses, largest first (only workloads fast on production JuiceFS)

| Workload | ext4-loop-unsafe | fxdax-dax | fxdax slower by |
|---|---|---|---|
| syncfs after a 4 KiB write | 23,800/s | 5.2/s | 4,610× |
| ws-2x-rand-read (2 GiB set, 2× daemon memory) | 189,100 IOPS | 66 IOPS | 2,870× |
| sync after a 4 KiB write | 5,455/s | 4.2/s | 1,309× |
| ws-4x-rand-read (4 GiB set) | 35,060 IOPS | 43 IOPS | 815× |
| freeze+thaw after a 4 KiB write | 1,608/s | 3.8/s | 428× |
| ws-1x-rand-read (1 GiB set) | 751,700 IOPS | 19,250 IOPS | 39× |
| rand-read-cold (4 KiB) | 61,830 IOPS | 4,223 IOPS | 14.6× |
| sqlite-pread-scan (cold) | 2,619 MiB/s | 257 MiB/s | 10.2× |
| seq-read-cold | 3,938 MiB/s | 1,552 MiB/s | 2.5× |
| read-small-cold | 41,280/s | 16,780/s | 2.5× |
| setxattr | 334,500/s | 143,300/s | 2.3× |
| fsync-append | 13,840 IOPS | 6,077 IOPS | 2.3× |
| find-cold | 218,700/s | 117,900/s | 1.9× |
| pgbench (tps) | 6,133 | 3,482 | 1.8× |
| stat-parallel | 1,024,000/s | 586,500/s | 1.8× |
| create-fsync | 7,772/s | 4,551/s | 1.7× |
| unlink-shared-dir | 239,800/s | 143,300/s | 1.7× |
| large-dir-stat-cold | 194,700/s | 132,300/s | 1.5× |

Every one of these, the two sides' ranges apart. seq-write (2.5×) and rand-write (9.3×) lost too,
but JuiceFS is slow at those (out of scope, user 2026-11-11).

## What the numbers say

- **Past the daemon's memory, every 4 KiB read costs ~30 ms** (p50 2.8 ms–29.8 ms, p99 32 ms at
  2× and 4×): about the bucket's injected 25 ms plus a 1 MiB part's decompression, as if each
  miss went to the bucket rather than the part cache on disk, for a set written seconds before
  (not yet confirmed from the daemon's counters). ext4 keeps
  serving from guest and host page cache (p99 20–67 µs).
- **sync, syncfs and freeze take ~200–270 ms each**: each waits for the upload and the lease
  (several bucket round trips at 25 ms), where with flushes dropped they wait for nothing.
- **Cold reads** inside the daemon's memory still lose 2.5–15×: a miss in the guest is a DAX
  fault through the daemon.
- pgbench and the fsync/metadata workloads lose 1.3–2.3×.

## Not measured in this run

fxdax without DAX; bucket latencies of 0 and 80 ms; apt and a syncing build (TASK-350.4,
TASK-350.5 AC2); npm, git on Linux (TASK-350.6); guest RAM idle under DAX (TASK-350.8).
