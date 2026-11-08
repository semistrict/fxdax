TASK-41: pgbench on Fxfs (guest kernel, DAX) with the store in GCS against ext4 on DAX pmem, at main 1a3391d7c plus task-perf-small (2161e4da2), as gce-fs-bench-pgbench-gcs-618

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench. 

## Result (TASK-41)

On a c3-standard-8 (us-east1-b, the bucket in us-east4), 3 iterations: pgbench-tps 2,174 on
fxfs-guest-dax against 1,826 on ext4-dax-pmem (1.19x ext4's), pgbench-init 1.70 s against 3.29 s
(0.52x). TASK-41's acceptance holds: tps at least ext4-dax-pmem's, init no longer.

## What made the difference

On 2026-10-21 (TASK-41's notes) Fxfs trailed at 1,366 tps against ext4's 1,880, bound by the
guest's CPU, each fsync an ordered commit of ~200 us in extentd and a WRITE request of ~280 us
guest-side. Here the daemon's own counters (comparison.md) put a commit at 40.4 us (committing the
store 11.3 us, waiting for object storage 28.9 us) and a WRITE at 27.5 us. Not bisected; the work
between on that path:
- writes through the DAX window: a write(2) makes the pages it writes private before taking the
  inode's lock (dfd9df24f), an append makes the pages ahead private in the PRIVATE it needs
  (f485ccb2f), and a PRIVATE's pages are made in one change of the window (99e68ad10);
- commits: a flush waits for the framings of the segments closed since the commit before rather
  than syncing their raw files too (e22ca2bc1), a framing syncs its own extents together
  (62f4d65e4) and writes back a MiB at a time (1c46c7dba).
