Perf batch 2 (TASK-41): the VMM's blocking calls to extentd spin 50 us for their reply before sleeping; on batch 1.

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench,metadata,io. 

Not kept: pgbench 1,407 tps against batch 1's 1,441 (noise), for CPU spun on every request; batch 1 alone is what landed (49dbcd3).
