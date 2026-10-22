TASK-58: pgbench -i -s 50 on Fxfs (DAX, GCS bucket) with a truncate's continued transaction's metadata reservation renewed; main's kernel panicked 30 s in (Insufficient reservation space).

Iterations: 1. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench. 
