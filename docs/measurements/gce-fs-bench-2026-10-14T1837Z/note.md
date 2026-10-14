Guest kernel Fxfs (page cache and DAX) against ext4 on the same kernel, after the fixes the first run found; new metadata workloads.

Iterations: 3. Cases: bench-fs-ext4 bench-fs-ext4-guest-kernel bench-fs-fxfs-guest bench-fs-fxfs-guest-dax bench-fs-fxdax-dax. Workloads: all.

bench-fs-fxdax-dax stopped at the new setxattr workload: fxdax's virtio-fs has no extended attributes. The benchmark now records the workload as failed there and goes on.
