# TASK-363: fxdax with cache_type Unsafe against ext4 on a loop device, Unsafe

ubu1, 2026-11-11. Tree 749051682 (main), guest kernel ~/src/t350-kernel (its sources unchanged
since). Three rounds of each, `scripts/bench-rounds.sh` under `fxjob --bench`, now isolated
(cpuset partition of cores 4-7, 12 of 16 L3 ways, the rest's memory bandwidth throttled; ubu1's
system-setup.d/146-fxdax_cgroups.sh), so no load gate. Not interleaved: the ext4 rounds ran
19:57-20:37Z, the fxdax ones after (the first fxdax rounds booted a Firecracker without
cache_type: main's submodule pointer was undone, 07c1d9c16, then restored, cf8fe5a62).
Same setup as [the governed run](../ubu1-loop-baseline-2026-11-11-governed/README.md): 2 vCPUs,
2 GiB VMs, groups io, metadata, working-set, sync and pgbench (20 s), fxdax's daemon governed,
file bucket on ZFS with 25 ms latency. `ab.md`: above 1, fxdax is slower.

## What Unsafe changed (fxdax durable, governed run, vs fxdax Unsafe)

| Workload | fxdax durable | fxdax Unsafe | ext4-loop-unsafe | fxdax Unsafe vs ext4 |
|---|---|---|---|---|
| sync loop | 3.2/s | 72/s | 5,314/s | 74× |
| syncfs loop | 3.9/s | 3,494/s | 27,770/s | 8× |
| freeze loop | 3.1/s | 450/s | 1,678/s | 3.7× |

## Still slower than ext4 Unsafe, largest first

rand-write 8.0× (JuiceFS's own weak spot), sqlite-pread-scan (cold) 5.4×, seq-read-cold 3.1×,
seq-write 3.0×, create-fsync 2.6×, fsync-append 2.5×, setxattr 2.4×, stat-parallel 2.1×,
pgbench-init 2.0×, pgbench-tps 1.8×, find-cold 1.9×, rand-read-cold 1.6×.

fxdax ahead where the working set passes the guest's memory: ws-2x 3.6×, ws-4x 4.2× faster.

## Correction (2026-11-12): sync alone

The sync group run alone (branch commit e48bfc57, `FXDAX_BENCH_GROUPS=sync`, one round) gave
fxdax Unsafe sync 3,470/s, syncfs 3,255/s, freeze 507/s: sync is 1.5x behind ext4 Unsafe's
5,314/s, not 74x. The 72/s above came after the io, metadata and working-set groups, whose
leftover work sync(2), unlike syncfs, waits for: 50 rounds at 14 ms is 0.7 s in all. That cause is
not confirmed. The durable case's 3/s is TASK-371.
