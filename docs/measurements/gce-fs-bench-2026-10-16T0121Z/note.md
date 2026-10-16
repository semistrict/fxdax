TASK-27.3: the guest kernel's Fxfs against ext4, all under Firecracker (vendor/firecracker, fxdax feature) rather than libkrun; compare with gce-fs-bench-2026-10-15T1720Z (libkrun).

Iterations: 3. Cases: bench-fs-ext4-guest-kernel bench-fs-ext4-dax-pmem bench-fs-fxfs-guest bench-fs-fxfs-guest-dax. Workloads: all. 
