TASK-26.15/26.16: guest window ranges outlive inodes; small files DAX, sharing staging extents (regions and slices); TreeCache shards; skip list levels for 2^20 items. Against ext4 DAX on writable pmem, the goal's baseline.

Iterations: 5. Cases: bench-fs-ext4-dax-pmem bench-fs-ext4-guest-kernel bench-fs-fxfs-guest-dax. Workloads: all. 
