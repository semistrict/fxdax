Every case and workload, 3 iterations, after TASK-25.9 (FUSE caching) and TASK-25.10 (guest halt polling).

Iterations: 3. Cases: all. Workloads: all.

Against 2026-10-10T2124Z: without DAX, mmap random reads 9,632 -> 1.20 million a second (ext4's
rate), cold sequential reads 1,652 -> 2,286 MiB/s, cold random reads 9,288 -> 12,047, creates
1,547 -> 2,909, cold stats 4,958 -> 5,753, unlinks 3,523 -> 4,041, the build 7.77 -> 7.29 s. With
DAX, creates 1,545 -> 2,837, cold stats 5,004 -> 6,044, unlinks 3,457 -> 3,900. Every I/O
workload but cold sequential reads through DAX (3.5x) is within 2x of ext4; the metadata
workloads remain 8-55x, passthrough's 6-36x.
