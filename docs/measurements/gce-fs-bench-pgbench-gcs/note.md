pgbench: Fxfs (guest kernel, DAX) on an extent store kept in GCS (gs://echophase-fxdax-bench, us-east4; durable commits wait for uploads), zstd-compressed segments uploaded as they are, against ext4 on DAX pmem.

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench. 
