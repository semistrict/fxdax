Guest kernel Fxfs (page cache and DAX) against ext4 on the same kernel; new metadata workloads.

Iterations: 3. Cases: bench-fs-ext4 bench-fs-ext4-guest-kernel bench-fs-fxfs-guest bench-fs-fxfs-guest-dax bench-fs-fxdax-dax. Workloads: all.

Incomplete: bench-fs-fxfs-guest failed in its second iteration (a write to an extent sealed between its requests, fixed in 41eec5a), and bench-fs-fxdax-dax in its first (compaction out of metadata space and an unreadable overwrite bitmap, fixed in 7de244e and 1b6c9ef). The comparison holds the cases that finished: ext4 on both kernels and the guest's Fxfs with DAX.
