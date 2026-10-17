pgbench on the 6.18 guest kernel (Amazon Linux kernel6.18-6.18.54-121.166) and Firecracker v1.16.1 with fxdax's series (TASK-39.6): Fxfs (guest kernel, DAX) on an extent store kept in GCS (gs://echophase-fxdax-bench, us-east4; durable commits wait for uploads), against ext4 on DAX pmem. Compare docs/measurements/gce-fs-bench-pgbench-gcs (6.12, libkrun).

Iterations: 3. Cases: bench-fs-ext4-dax-pmem bench-fs-fxfs-guest-dax. Workloads: pgbench. 
