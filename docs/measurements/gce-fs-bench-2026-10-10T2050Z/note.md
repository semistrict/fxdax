TASK-25.4: the host MMU changes protections in runs of consecutive pages, one host and one second stage call each, and not at all where they stay, for fewer TLB flushes; DAX against no DAX.

Iterations: 3. Cases: bench-fs-fxdax-dax bench-fs-fxdax-nodax. Workloads: io.

Finding: sequential DAX writes at 60 MiB/s, against 58 without DAX (47 in 2026-10-10T2043Z);
mmap random writes at 4,562 a second, against ext4's ~6,350.
