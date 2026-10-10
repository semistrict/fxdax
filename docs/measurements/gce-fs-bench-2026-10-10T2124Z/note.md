Every case and workload, 3 iterations, after TASK-25.2 to 25.6: where fxdax stands against ext4 and passthrough now, against the baseline 2026-10-10T1823Z.

Iterations: 3. Cases: all. Workloads: all.

Against the baseline (2026-10-10T1823Z), fxdax with DAX: sequential writes 34.6 -> 58.2 MiB/s,
cold random reads 8,943 -> 32,508 a second (ext4 13,932), cold sequential reads 172 -> 865 MiB/s,
mmap random reads 13,395 -> 1.11 million a second (ext4 1.20 million), mmap random writes 1,548
-> 4,579, creates 1,211 -> 1,545, cold stats 2,640 -> 5,004, unlinks 2,755 -> 3,457. Without DAX:
cold stats 2,572 -> 4,958 (1.97x), creates 1,184 -> 1,547, unlinks 2,835 -> 3,523. Files are
stamped within 0.13 ms of the guest's clock (170 ms before). Over 2x ext4 still: cold sequential
reads through DAX (4.1x), and the metadata workloads on every virtio-fs backend.
