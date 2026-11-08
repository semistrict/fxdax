TASK-91.8: the sandbox team's baselines, ext4 on virtio-blk with cache_type Unsafe and Writeback, beside ext4-dax-pmem and the guest's Fxfs without DAX and with it; pgbench at scale 50, two 60 s runs a VM, its bucket 25 ms away, jailed

Iterations: 1. Cases: pgbench-ext4-blk-unsafe pgbench-ext4-blk-writeback pgbench-ext4-dax-pmem fxfs-guest-pgbench fxfs-guest-pgbench-dax. Workloads: all. 

## Result (TASK-91.8)

GCE c3-standard-8 (fxdax-bench-r1022, us-east1-b), each case one VM of 2 vCPUs, its database
loaded at scale 50 then two 60 s runs of `pgbench -c 4 -j 2`, the Fxfs cases' bucket a file
bucket 25 ms away, every VM jailed: the perf gate's pgbench cases with FXDAX_PGBENCH_RUNS=2
(bench-gce.sh naming them; its fs-bench report has none of theirs). steps.md and steps.json are
scripts/pgbench-steps.py's report of the logs, step by step: the load's phases; each run's tps,
mean latency and the guest's CPU; and, for the Fxfs cases, the extent daemon's requests and
commits over each run.

| Case | Load | Run 1 | Run 2 | Guest idle |
|---|---|---|---|---|
| ext4 on virtio-blk, flushes dropped (Unsafe) | 10.87 s | 3,609 tps | 3,820 tps | 9% |
| ext4 on virtio-blk, each flush a host fsync (Writeback) | 15.16 s | 1,536 tps | 1,534 tps | 55-57% |
| ext4 with DAX on virtio-pmem | 17.48 s | 1,263 tps | 1,163 tps | 57-60% |
| Fxfs, no DAX | 13.46 s | 2,011 tps | 2,037 tps | 18-19% |
| Fxfs, DAX | 13.45 s | 2,004 tps | 2,047 tps | 17-18% |

Fxfs keeps every commit durable and is 1.3x the durable baseline (Writeback), at 0.55x the
sandbox team's runtime, which drops flushes; its guest is CPU-bound (idle 17-19%, against
Unsafe's 9%), where both durable ext4 cases wait on their devices. Its daemon served ~3,340
WRITEs a second (31-38 us each) and ~1,370 commits a second, each waiting 5-20 us for object
storage on average. The Fxfs cases' file buckets are not kept here.
